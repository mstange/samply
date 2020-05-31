use super::kernel_error::{self, IntoResult};
use libc::strlen;
use mach;
use mach::message::mach_msg_type_number_t;
use mach::port::mach_port_t;
use mach::task::{task_info, task_resume, task_suspend};
use mach::task_info::{task_info_t, TASK_DYLD_INFO};
use mach::thread_act::{thread_get_state, thread_resume, thread_suspend};
use mach::thread_status::{thread_state_t, x86_THREAD_STATE64};
use mach::traps::mach_task_self;
use mach::vm::{mach_vm_deallocate, mach_vm_read, mach_vm_read_overwrite, mach_vm_remap};
use mach::vm_inherit::VM_INHERIT_SHARE;
use mach::vm_page_size::{mach_vm_trunc_page, vm_page_size};
use mach::vm_prot::{vm_prot_t, VM_PROT_NONE};
use mach::vm_types::{mach_vm_address_t, mach_vm_size_t};
use std::cmp::Ordering;
use std::mem;
use std::ptr;
use uuid::Uuid;

use mach::structs::x86_thread_state64_t;

use crate::dyld_bindings;
use dyld_bindings::{
    dyld_all_image_infos, dyld_image_info, load_command, mach_header_64, segment_command_64,
    uuid_command,
};

#[derive(Debug, Clone)]
pub struct ThreadInfo {
    pub tid: u64,
    pub name: String,
    pub backtrace: Option<Vec<u64>>,
}

#[derive(Debug, Clone)]
pub struct DyldInfo {
    pub file: String,
    pub address: u64,
    pub vmsize: u64,
    pub uuid: Option<Uuid>,
}

pub fn get_dyld_info(task: mach_port_t) -> kernel_error::Result<Vec<DyldInfo>> {
    unsafe { task_suspend(task) }.into_result()?;
    let result = get_dyld_info_with_suspended_task(task);
    let _ = unsafe { task_resume(task) };
    result
}

pub fn get_dyld_info_with_suspended_task(task: mach_port_t) -> kernel_error::Result<Vec<DyldInfo>> {
    // Adapted from rbspy and from the Gecko profiler's shared-libraries-macos.cc.

    let mut vec = Vec::new();

    let mut dyld_info = task_dyld_info {
        all_image_info_addr: 0,
        all_image_info_size: 0,
        all_image_info_format: 0,
    };

    // TASK_DYLD_INFO_COUNT is #define'd to be 5 in /usr/include/mach/task_info.h
    // ... doesn't seem to be included in the mach crate =(
    let mut count: mach_msg_type_number_t = 5;
    unsafe {
        task_info(
            task,
            TASK_DYLD_INFO,
            &mut dyld_info as *mut task_dyld_info as task_info_t,
            &mut count,
        )
    }
    .into_result()?;

    // Read in the dyld_all_image_infos information here here.
    let mut image_infos = dyld_all_image_infos::default();
    let mut read_len = std::mem::size_of_val(&image_infos) as mach_vm_size_t;

    unsafe {
        // While we could use the read_process_memory crate for this, this adds a dependency
        // for something that is pretty trivial
        mach_vm_read_overwrite(
            task,
            dyld_info.all_image_info_addr,
            read_len,
            (&mut image_infos) as *mut dyld_all_image_infos as mach_vm_address_t,
            &mut read_len,
        )
    }
    .into_result()?;

    // copy the infoArray element of dyld_all_image_infos ovber
    let mut modules = vec![dyld_image_info::default(); image_infos.infoArrayCount as usize];
    let mut read_len = (std::mem::size_of::<dyld_image_info>()
        * image_infos.infoArrayCount as usize) as mach_vm_size_t;
    unsafe {
        mach_vm_read_overwrite(
            task,
            image_infos.infoArray as mach_vm_address_t,
            read_len,
            modules.as_mut_ptr() as mach_vm_address_t,
            &mut read_len,
        )
    }
    .into_result()?;

    for module in modules {
        let mut read_len = 512 as mach_vm_size_t;
        let mut image_filename = [0_i8; 512];
        unsafe {
            mach_vm_read_overwrite(
                task,
                module.imageFilePath as mach_vm_address_t,
                read_len,
                image_filename.as_mut_ptr() as mach_vm_address_t,
                &mut read_len,
            )
        }
        .into_result()?;

        let ptr = image_filename.as_ptr();
        let slice = unsafe { std::slice::from_raw_parts(ptr as *mut u8, strlen(ptr)) };
        let filename = std::str::from_utf8(slice).unwrap().to_owned();

        // read in the mach header
        let mut header = mach_header_64::default();
        let mut read_len = std::mem::size_of_val(&header) as mach_vm_size_t;
        unsafe {
            mach_vm_read_overwrite(
                task,
                module.imageLoadAddress as u64,
                read_len,
                (&mut header) as *mut mach_header_64 as mach_vm_address_t,
                &mut read_len,
            )
        }
        .into_result()?;

        let mut commands_buffer = vec![0_i8; header.sizeofcmds as usize];
        let mut read_len = mach_vm_size_t::from(header.sizeofcmds);
        unsafe {
            mach_vm_read_overwrite(
                task,
                (module.imageLoadAddress as usize + std::mem::size_of_val(&header))
                    as mach_vm_size_t,
                read_len,
                commands_buffer.as_mut_ptr() as mach_vm_address_t,
                &mut read_len,
            )
        }
        .into_result()?;

        // Figure out the slide from the __TEXT segment if appropiate
        let mut vmsize: u64 = 0;
        let mut uuid = None;
        let mut offset = 0;
        for _ in 0..header.ncmds {
            unsafe {
                let command =
                    commands_buffer.as_ptr().offset(offset as isize) as *const load_command;
                match (*command).cmd {
                    0x19 => {
                        // LC_SEGMENT_64
                        let segcmd = command as *const segment_command_64;
                        if (*segcmd).segname[0..7] == [95, 95, 84, 69, 88, 84, 0] {
                            // This is the __TEXT segment.
                            vmsize = (*segcmd).vmsize;
                        }
                    }
                    0x1b => {
                        // LC_UUID
                        let ucmd = command as *const uuid_command;
                        uuid = Some(Uuid::from_slice(&(*ucmd).uuid).unwrap());
                    }
                    _ => {}
                }
                offset += (*command).cmdsize;
            }
        }
        vec.push(DyldInfo {
            file: filename.clone(),
            address: module.imageLoadAddress as u64,
            vmsize,
            uuid,
        });
    }
    vec.sort_by_key(|info| info.address);
    Ok(vec)
}

// bindgen seemed to put all the members for this struct as a single opaque blob:
//      (bindgen /usr/include/mach/task_info.h --with-derive-default --whitelist-type task_dyld_info)
// rather than debug the bindgen command, just define manually here
#[repr(C)]
#[derive(Default, Debug)]
pub struct task_dyld_info {
    pub all_image_info_addr: mach_vm_address_t,
    pub all_image_info_size: mach_vm_size_t,
    pub all_image_info_format: mach::vm_types::integer_t,
}

pub fn get_backtrace(
    memory: &mut ForeignMemory,
    thread_act: mach_port_t,
    frames: &mut Vec<u64>,
) -> kernel_error::Result<()> {
    unsafe { thread_suspend(thread_act) }.into_result()?;

    let mut state: x86_thread_state64_t = unsafe { mem::zeroed() };
    let mut count = x86_thread_state64_t::count();
    let res = unsafe {
        thread_get_state(
            thread_act,
            x86_THREAD_STATE64,
            &mut state as *mut _ as thread_state_t,
            &mut count as *mut _,
        )
    }
    .into_result();

    if let Err(err) = res {
        let _ = unsafe { thread_resume(thread_act) };
        return Err(err);
    }

    do_frame_pointer_stackwalk(&state, memory, frames);

    let _ = unsafe { thread_resume(thread_act) };

    Ok(())
}

fn do_frame_pointer_stackwalk(
    initial_state: &x86_thread_state64_t,
    memory: &mut ForeignMemory,
    frames: &mut Vec<u64>,
) {
    frames.push(initial_state.__rip);

    // Do a frame pointer stack walk. Code that is compiled with frame pointers
    // has the following function prologues and epilogues:
    //
    // Function prologue:
    // pushq  %rbp
    // movq   %rsp, %rbp
    //
    // Function epilogue:
    // popq   %rbp
    // ret
    //
    // Functions are called with callq; callq pushes the return address onto the stack.
    // When a function reaches its end, ret pops the return address from the stack and jumps to it.
    // So when a function is called, we have the following stack layout:
    //
    //                                                                     [... rest of the stack]
    //                                                                     ^ rsp           ^ rbp
    //     callq some_function
    //                                                   [return address]  [... rest of the stack]
    //                                                   ^ rsp                             ^ rbp
    //     pushq %rbp
    //                         [caller's frame pointer]  [return address]  [... rest of the stack]
    //                         ^ rsp                                                       ^ rbp
    //     movq %rsp, %rbp
    //                         [caller's frame pointer]  [return address]  [... rest of the stack]
    //                         ^ rsp, rbp
    //     <other instructions>
    //       [... more stack]  [caller's frame pointer]  [return address]  [... rest of the stack]
    //       ^ rsp             ^ rbp
    //
    // So: *rbp is the caller's frame pointer, and *(rbp + 8) is the return address.
    //
    // Or, in other words, the following linked list is built up on the stack:
    // #[repr(C)]
    // struct CallFrameInfo {
    //     previous: *const CallFrameInfo,
    //     return_address: *const c_void,
    // }
    // and rbp is a *const CallFrameInfo.

    let mut bp = initial_state.__rbp;
    while bp != 0 && (bp & 7) == 0 {
        let next = match memory.read_u64_at_address(bp) {
            Ok(val) => val,
            Err(_) => break, // usually KernelError::InvalidAddress
        };
        // The caller frame will always be lower on the stack (at a higher address)
        // than this frame. Make sure this is the case, so that we don't go in circles.
        if next <= bp {
            break;
        }
        let return_address = match memory.read_u64_at_address(bp + 8) {
            Ok(val) => val,
            Err(_) => break, // usually KernelError::InvalidAddress
        };
        frames.push(return_address);
        bp = next;
    }

    frames.reverse();
}

#[derive(Debug, Clone)]
pub struct ForeignMemory {
    task: mach_port_t,
    data: Vec<VmData>,
}

impl ForeignMemory {
    pub fn new(task: mach_port_t) -> Self {
        Self {
            task,
            data: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.data.clear();
        self.data.shrink_to_fit();
    }

    pub fn read_u64_at_address(&mut self, address: u64) -> kernel_error::Result<u64> {
        let search = self.data.binary_search_by(|d| {
            if d.address_range.start > address {
                Ordering::Greater
            } else if d.address_range.end <= address {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        });
        let vm_data = match search {
            Ok(i) => &self.data[i],
            Err(i) => {
                let start_addr = unsafe { mach_vm_trunc_page(address) };
                let size = unsafe { vm_page_size } as u64;
                let data = VmData::map_from_task(self.task, start_addr, size)?;
                self.data.insert(i, data);
                &self.data[i]
            }
        };
        Ok(vm_data.read_u64_at_address(address))
    }
}

#[derive(Debug, Clone)]
struct VmData {
    address_range: std::ops::Range<u64>,
    data: *mut u8,
    data_size: usize,
}

impl VmData {
    #[allow(unused)]
    pub fn read_from_task(
        task: mach_port_t,
        original_address: u64,
        size: u64,
    ) -> kernel_error::Result<Self> {
        let mut data: *mut u8 = ptr::null_mut();
        let mut data_size: usize = 0;
        unsafe {
            mach_vm_read(
                task,
                original_address,
                size,
                mem::transmute(&mut data),
                mem::transmute(&mut data_size),
            )
        }
        .into_result()?;

        Ok(Self {
            address_range: original_address..(original_address + data_size as u64),
            data,
            data_size,
        })
    }

    pub fn map_from_task(
        task: mach_port_t,
        original_address: u64,
        size: u64,
    ) -> kernel_error::Result<Self> {
        let mut data: *mut u8 = ptr::null_mut();
        let mut cur_protection: vm_prot_t = VM_PROT_NONE;
        let mut max_protection: vm_prot_t = VM_PROT_NONE;
        unsafe {
            mach_vm_remap(
                mach_task_self(),
                mem::transmute(&mut data),
                size,
                0,
                1, /* anywhere: true */
                task,
                original_address,
                0,
                mem::transmute(&mut cur_protection),
                mem::transmute(&mut max_protection),
                VM_INHERIT_SHARE,
            )
        }
        .into_result()?;

        Ok(Self {
            address_range: original_address..(original_address + size),
            data,
            data_size: size as usize,
        })
    }

    pub fn read_u64_at_address(&self, address: u64) -> u64 {
        if address < self.address_range.start {
            panic!(
                "address {} is before the range that we read (which starts at {})",
                address, self.address_range.start
            );
        }
        if address >= self.address_range.end {
            panic!(
                "address {} is after the range that we read (which ends at {})",
                address, self.address_range.end
            );
        }
        let ptr = unsafe {
            self.data
                .offset((address - self.address_range.start) as isize)
        };
        unsafe { *(ptr as *const u8 as *const u64) }
    }
}

impl Drop for VmData {
    fn drop(&mut self) {
        let _ = unsafe {
            mach_vm_deallocate(
                mach_task_self(),
                self.data as *mut _ as _,
                self.data_size as _,
            )
        };
    }
}
