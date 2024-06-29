use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::{Deref, Range};
use std::{mem, ptr};

use dyld_bindings::{dyld_all_image_infos, dyld_image_info};
#[cfg(target_arch = "aarch64")]
use framehop::aarch64::PtrAuthMask;
#[cfg(target_arch = "aarch64")]
use framehop::aarch64::UnwindRegsAarch64;
#[cfg(target_arch = "x86_64")]
use framehop::x86_64::UnwindRegsX86_64;
use framehop::{FrameAddress, UnwindRegsNative};
use fxprof_processed_profile::debugid::DebugId;
use mach::message::mach_msg_type_number_t;
use mach::port::mach_port_t;
use mach::task::{task_info, task_resume, task_suspend};
use mach::task_info::{task_info_t, TASK_DYLD_INFO};
use mach::thread_act::{thread_get_state, thread_resume, thread_suspend};
#[cfg(target_arch = "aarch64")]
use mach::thread_status::thread_state_flavor_t;
use mach::thread_status::thread_state_t;
use mach::traps::mach_task_self;
use mach::vm::{mach_vm_deallocate, mach_vm_read, mach_vm_remap};
use mach::vm_inherit::VM_INHERIT_SHARE;
use mach::vm_page_size::{mach_vm_trunc_page, vm_page_size};
use mach::vm_prot::{vm_prot_t, VM_PROT_NONE, VM_PROT_READ};
use mach::vm_types::{mach_vm_address_t, mach_vm_size_t};
#[cfg(target_arch = "x86_64")]
use mach::{structs::x86_thread_state64_t, thread_status::x86_THREAD_STATE64};
use object::macho::{
    MachHeader64, SegmentCommand64, CPU_SUBTYPE_ARM64E, CPU_SUBTYPE_ARM64_ALL, CPU_SUBTYPE_MASK,
    CPU_SUBTYPE_X86_64_ALL, CPU_SUBTYPE_X86_64_H, CPU_TYPE_ARM64, CPU_TYPE_X86_64, MH_EXECUTE,
};
use object::read::macho::{MachHeader, Section, Segment};
use object::LittleEndian;
#[cfg(target_arch = "aarch64")]
use once_cell::sync::Lazy;
use uuid::Uuid;
use wholesym::samply_symbols::object;
use wholesym::CodeId;

use super::dyld_bindings::{self};
use super::error::SamplingError;
use super::kernel_error::{self, IntoResult, KernelError};
use super::task_profiler::UnwindSectionBytes;

pub const TASK_DYLD_INFO_COUNT: mach_msg_type_number_t = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DyldInfo {
    pub is_executable: bool,
    pub file: String,
    pub base_avma: u64,
    pub vmsize: u64,
    pub module_info: ModuleSvmaInfo,
    pub debug_id: Option<DebugId>,
    pub code_id: Option<CodeId>,
    pub arch: Option<&'static str>,
    pub unwind_sections: UnwindSectionInfo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSvmaInfo {
    pub base_svma: u64,
    pub text_svma: Option<Range<u64>>,
    pub stubs_svma: Option<Range<u64>>,
    pub stub_helper_svma: Option<Range<u64>>,
    pub got_svma: Option<Range<u64>>,
    pub eh_frame_svma: Option<Range<u64>>,
    pub eh_frame_hdr_svma: Option<Range<u64>>,
    pub text_segment_svma: Option<Range<u64>>,
}

/// These are SVMAs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnwindSectionInfo {
    /// (address, size)
    pub unwind_info_section: Option<(u64, u64)>,
    /// (address, size)
    pub eh_frame_section: Option<(u64, u64)>,
    /// (address, size)
    pub text_segment: Option<(u64, u64)>,
}

pub struct DyldInfoManager {
    task: mach_port_t,
    memory: ForeignMemory,
    all_image_info_addr: Option<u64>,
    last_change_timestamp: Option<u64>,
    saved_image_info: Vec<DyldInfo>,
}

impl DyldInfoManager {
    pub fn new(task: mach_port_t) -> DyldInfoManager {
        DyldInfoManager {
            task,
            memory: ForeignMemory::new(task),
            all_image_info_addr: None,
            last_change_timestamp: None,
            saved_image_info: Vec::new(),
        }
    }

    pub fn unmap_memory(&mut self) {
        self.memory.clear();
    }

    pub fn check_for_changes(&mut self) -> kernel_error::Result<Vec<Modification<DyldInfo>>> {
        // Avoid suspending the task if we know that the image info array hasn't changed.
        // The process-wide dyld_all_image_infos instance always stays in the same place,
        // so we can keep its memory mapped and just check the timestamp in the mapped memory.
        if let (Some(last_change_timestamp), Some(info_addr)) =
            (self.last_change_timestamp, self.all_image_info_addr)
        {
            let image_infos: &dyld_all_image_infos =
                unsafe { self.memory.get_type_ref_at_address(info_addr) }?;
            // infoArrayChangeTimestamp is 10.12+. TODO: check version
            if image_infos.infoArrayChangeTimestamp == last_change_timestamp {
                return Ok(Vec::new());
            }
        }

        // Now, suspend the task, enumerate the libraries, and diff against our saved list.
        with_suspended_task(self.task, || {
            let info_addr = match self.all_image_info_addr {
                Some(addr) => addr,
                None => get_all_image_info_addr(self.task)?,
            };

            self.all_image_info_addr = Some(info_addr);

            let (
                info_array_addr,
                info_array_count,
                info_array_change_timestamp,
                dyld_image_load_addr,
                dyld_image_path,
            ) = {
                let image_infos: &dyld_all_image_infos =
                    unsafe { self.memory.get_type_ref_at_address(info_addr) }?;
                (
                    image_infos.infoArray as usize as u64,
                    image_infos.infoArrayCount,
                    image_infos.infoArrayChangeTimestamp, // 10.12+
                    image_infos.dyldImageLoadAddress as usize as u64,
                    image_infos.dyldPath as usize as u64, // 10.12+
                )
            };

            // From dyld_images.h:
            // For a snashot of what images are currently loaded, the infoArray fields contain a pointer
            // to an array of all images. If infoArray is NULL, it means it is being modified, come back later.
            if info_array_addr == 0 {
                // Pretend there are no modifications. We will pick up the modifications the next time we're called.
                return Ok(Vec::new());
            }

            let new_image_info = enumerate_dyld_images(
                &mut self.memory,
                info_array_addr,
                info_array_count,
                dyld_image_load_addr,
                dyld_image_path,
            )?;

            // self.saved_image_info and new_image_info are sorted by address. Diff the two lists.
            let diff =
                diff_sorted_slices(&self.saved_image_info, &new_image_info, |left, right| {
                    left.base_avma.cmp(&right.base_avma)
                });

            self.last_change_timestamp = Some(info_array_change_timestamp);
            self.saved_image_info = new_image_info;

            Ok(diff)
        })
    }
}

fn get_all_image_info_addr(task: mach_port_t) -> kernel_error::Result<u64> {
    let mut dyld_info = task_dyld_info {
        all_image_info_addr: 0,
        all_image_info_size: 0,
        all_image_info_format: 0,
    };

    let mut count = TASK_DYLD_INFO_COUNT;
    unsafe {
        task_info(
            task,
            TASK_DYLD_INFO,
            &mut dyld_info as *mut task_dyld_info as task_info_t,
            &mut count,
        )
    }
    .into_result()?;

    Ok(dyld_info.all_image_info_addr)
}

fn with_suspended_task<T>(
    task: mach_port_t,
    f: impl FnOnce() -> kernel_error::Result<T>,
) -> kernel_error::Result<T> {
    unsafe { task_suspend(task) }.into_result()?;
    let result = f();
    let _ = unsafe { task_resume(task) };
    result
}

fn enumerate_dyld_images(
    memory: &mut ForeignMemory,
    info_array_addr: u64,
    info_array_count: u32,
    dyld_image_load_addr: u64,
    dyld_image_path: u64,
) -> kernel_error::Result<Vec<DyldInfo>> {
    // Adapted from rbspy and from the Gecko profiler's shared-libraries-macos.cc.
    let mut vec = vec![get_dyld_image_info(
        memory,
        dyld_image_load_addr,
        dyld_image_path,
    )?];

    for image_index in 0..info_array_count {
        let (base_avma, image_file_path) = {
            let info_array_elem_addr =
                info_array_addr + image_index as u64 * mem::size_of::<dyld_image_info>() as u64;
            let image_info: &dyld_image_info =
                unsafe { memory.get_type_ref_at_address(info_array_elem_addr) }?;
            (
                image_info.imageLoadAddress as usize as u64,
                image_info.imageFilePath as usize as u64,
            )
        };
        vec.push(get_dyld_image_info(memory, base_avma, image_file_path)?);
    }
    vec.sort_by_key(|info| info.base_avma);
    Ok(vec)
}

fn get_arch_string(cputype: u32, cpusubtype: u32) -> Option<&'static str> {
    let s = match (cputype, cpusubtype & !CPU_SUBTYPE_MASK) {
        (CPU_TYPE_X86_64, CPU_SUBTYPE_X86_64_ALL) => "x86_64",
        (CPU_TYPE_X86_64, CPU_SUBTYPE_X86_64_H) => "x86_64h",
        (CPU_TYPE_ARM64, CPU_SUBTYPE_ARM64_ALL) => "arm64",
        (CPU_TYPE_ARM64, CPU_SUBTYPE_ARM64E) => "arm64e",
        _ => return None,
    };
    Some(s)
}

fn get_dyld_image_info(
    memory: &mut ForeignMemory,
    base_avma: u64,
    image_file_path: u64,
) -> kernel_error::Result<DyldInfo> {
    let filename = {
        let filename_bytes: &[i8; 512] =
            unsafe { memory.get_type_ref_at_address(image_file_path) }?;
        unsafe { std::ffi::CStr::from_ptr(filename_bytes.as_ptr()) }
            .to_string_lossy()
            .to_string()
    };

    let header = {
        let header: &MachHeader64<LittleEndian> =
            unsafe { memory.get_type_ref_at_address(base_avma) }?;
        *header
    };

    let endian = LittleEndian;
    let commands_start = base_avma + mem::size_of::<MachHeader64<LittleEndian>>() as u64;
    let commands_end = commands_start + header.sizeofcmds(endian) as u64;
    let header_and_command_data = memory.get_slice(base_avma..commands_end)?;
    let mut load_commands = header
        .load_commands(endian, header_and_command_data, 0)
        .map_err(|_| kernel_error::KernelError::InvalidValue)?;

    let mut base_svma = 0;
    let mut vmsize: u64 = 0;
    let mut uuid = None;
    let mut sections = HashMap::new();

    while let Ok(Some(command)) = load_commands.next() {
        if let Ok(Some((segment, section_data))) = SegmentCommand64::from_command(command) {
            if segment.name() == b"__TEXT" {
                base_svma = segment.vmaddr(endian);
                vmsize = segment.vmsize(endian);

                for section in segment
                    .sections(endian, section_data)
                    .map_err(|_| KernelError::InvalidArgument)?
                {
                    let addr = section.addr.get(endian);
                    let size = section.size.get(endian);
                    sections.insert(section.name(), (addr, size));
                }
            }
        } else if let Ok(Some(uuid_command)) = command.uuid() {
            uuid = Some(Uuid::from_bytes(uuid_command.uuid));
        }
    }

    let section_svma_range = |name: &[u8]| -> Option<Range<u64>> {
        sections.get(name).map(|(addr, size)| *addr..*addr + *size)
    };

    Ok(DyldInfo {
        file: filename,
        base_avma,
        vmsize,
        module_info: ModuleSvmaInfo {
            base_svma,
            text_svma: section_svma_range(b"__text"),
            stubs_svma: section_svma_range(b"__stubs"),
            stub_helper_svma: section_svma_range(b"__stub_helper"),
            got_svma: section_svma_range(b"__got"),
            eh_frame_svma: section_svma_range(b"__eh_frame"),
            eh_frame_hdr_svma: section_svma_range(b"__eh_frame_hdr"),
            text_segment_svma: Some(base_svma..base_svma + vmsize),
        },
        debug_id: uuid.map(DebugId::from_uuid),
        code_id: uuid.map(CodeId::MachoUuid),
        arch: get_arch_string(header.cputype(endian), header.cpusubtype(endian)),
        is_executable: header.filetype(endian) == MH_EXECUTE,
        unwind_sections: UnwindSectionInfo {
            unwind_info_section: sections.get(&b"__unwind_info"[..]).cloned(),
            eh_frame_section: sections.get(&b"__eh_frame"[..]).cloned(),
            text_segment: Some((base_svma, vmsize)),
        },
    })
}

// bindgen seemed to put all the members for this struct as a single opaque blob:
//      (bindgen /usr/include/mach/task_info.h --with-derive-default --whitelist-type task_dyld_info)
// rather than debug the bindgen command, just define manually here
#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Default, Debug)]
pub struct task_dyld_info {
    pub all_image_info_addr: mach_vm_address_t,
    pub all_image_info_size: mach_vm_size_t,
    pub all_image_info_format: mach::vm_types::integer_t,
}

#[allow(non_camel_case_types)]
#[cfg(target_arch = "aarch64")]
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Hash, PartialOrd, PartialEq, Eq, Ord)]
pub struct arm_thread_state64_t {
    pub __x: [u64; 29],
    pub __fp: u64, // frame pointer x29
    pub __lr: u64, // link register x30
    pub __sp: u64, // stack pointer x31
    pub __pc: u64,
    pub __cpsr: u32,
    pub __pad: u32,
}

#[cfg(target_arch = "aarch64")]
impl arm_thread_state64_t {
    pub fn count() -> mach_msg_type_number_t {
        (mem::size_of::<Self>() / mem::size_of::<u32>()) as mach_msg_type_number_t
    }
}

#[cfg(target_arch = "aarch64")]
pub static ARM_THREAD_STATE64: thread_state_flavor_t = 6;

#[cfg(target_arch = "x86_64")]
fn get_unwinding_registers(
    thread_act: mach_port_t,
) -> kernel_error::Result<(u64, UnwindRegsX86_64)> {
    let mut state: x86_thread_state64_t = unsafe { mem::zeroed() };
    let mut count = x86_thread_state64_t::count();
    unsafe {
        thread_get_state(
            thread_act,
            x86_THREAD_STATE64,
            &mut state as *mut _ as thread_state_t,
            &mut count as *mut _,
        )
    }
    .into_result()?;
    Ok((
        state.__rip,
        UnwindRegsX86_64::new(state.__rip, state.__rsp, state.__rbp),
    ))
}

#[cfg(target_arch = "aarch64")]
fn read_int_sysctl_by_name(name: &str) -> Result<i32, sysctl::SysctlError> {
    use sysctl::Sysctl;
    let val = sysctl::Ctl::new(name)?.value()?;
    let val = *val.as_int().ok_or(sysctl::SysctlError::ExtractionError)?;
    Ok(val)
}

pub fn proc_cmdline(pid: i32) -> Result<Vec<String>, sysctl::SysctlError> {
    unsafe {
        let mib: [i32; 3] = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let args: [u8; 65536] = std::mem::zeroed();
        let size: usize = std::mem::size_of_val(&args);
        let ret = libc::sysctl(
            &mib as *const _ as *mut _,
            3,
            &args as *const _ as *mut _,
            &size as *const _ as *mut _,
            std::ptr::null_mut(),
            0,
        );

        if ret < 0 {
            return Err(std::io::Error::last_os_error().into());
        }

        // get the number of arguments
        let argcount: i32 = *(&args as *const _ as *const i32);
        let args = &args[std::mem::size_of_val(&argcount)..];

        // split off of the exe from the beginning
        let args = &args[libc::strlen(args as *const _ as *const i8)..];

        let mut ret = Vec::new();
        for arg in args.split(|b| *b == 0) {
            // ignore leading nulls
            if arg.is_empty() && ret.is_empty() {
                continue;
            }

            let arg = String::from_utf8(arg.to_vec()).map_err(|e| e.utf8_error())?;

            ret.push(arg);
            if ret.len() >= argcount as usize {
                break;
            }
        }
        Ok(ret)
    }
}

#[cfg(target_arch = "aarch64")]
/// Read the `machdep.virtual_address_size` sysctl.
fn get_virtual_address_size() -> Option<u32> {
    let bitcount = match read_int_sysctl_by_name("machdep.virtual_address_size") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Reading machdep.virtual_address_size failed: {e}");
            return None;
        }
    };
    if bitcount <= 0 || bitcount > 64 {
        eprintln!("Unexpected machdep.virtual_address_size: {bitcount}");
        return None;
    }
    Some(bitcount as u32)
}

#[cfg(target_arch = "aarch64")]
static PTR_AUTH_MASK: Lazy<PtrAuthMask> = Lazy::new(|| {
    let addr_bits = get_virtual_address_size().unwrap_or(47);
    let mask_bits = 64 - addr_bits;
    PtrAuthMask(u64::MAX >> mask_bits)
});

#[cfg(target_arch = "aarch64")]
fn get_unwinding_registers(
    thread_act: mach_port_t,
) -> kernel_error::Result<(u64, UnwindRegsAarch64)> {
    let mut state: arm_thread_state64_t = unsafe { mem::zeroed() };
    let mut count = arm_thread_state64_t::count();
    unsafe {
        thread_get_state(
            thread_act,
            ARM_THREAD_STATE64,
            &mut state as *mut _ as thread_state_t,
            &mut count as *mut _,
        )
    }
    .into_result()?;
    let mask = *PTR_AUTH_MASK;
    Ok((
        mask.strip_ptr_auth(state.__pc),
        UnwindRegsAarch64::new_with_ptr_auth_mask(mask, state.__lr, state.__sp, state.__fp),
    ))
}

fn with_suspended_thread<R>(
    thread_act: mach_port_t,
    f: impl FnOnce() -> R,
) -> kernel_error::Result<R> {
    unsafe { thread_suspend(thread_act) }.into_result()?;
    let result = f();
    let _ = unsafe { thread_resume(thread_act) };
    Ok(result)
}

pub struct StackwalkerRef<'a> {
    unwinder: &'a framehop::UnwinderNative<UnwindSectionBytes, framehop::MayAllocateDuringUnwind>,
    cache: &'a mut framehop::CacheNative<framehop::MayAllocateDuringUnwind>,
}

impl<'a> StackwalkerRef<'a> {
    pub fn new(
        unwinder: &'a framehop::UnwinderNative<
            UnwindSectionBytes,
            framehop::MayAllocateDuringUnwind,
        >,
        cache: &'a mut framehop::CacheNative<framehop::MayAllocateDuringUnwind>,
    ) -> Self {
        Self { unwinder, cache }
    }
}

/// `frames` must be empty initially.
///
/// On return, `frames` will have the stack frames from callee-most to root-most.
pub fn get_backtrace(
    stackwalker: StackwalkerRef,
    memory: &mut ForeignMemory,
    thread_act: mach_port_t,
    frames: &mut Vec<FrameAddress>,
    fold_recursive_prefix: bool,
) -> Result<(), SamplingError> {
    with_suspended_thread(thread_act, || {
        let (pc, regs) = get_unwinding_registers(thread_act).map_err(|err| match err {
            KernelError::InvalidArgument
            | KernelError::MachSendInvalidDest
            | KernelError::Terminated => {
                SamplingError::ThreadTerminated("thread_get_state in get_unwinding_registers", err)
            }
            err => SamplingError::Ignorable("thread_get_state in get_unwinding_registers", err),
        })?;
        do_stackwalk(stackwalker, pc, regs, memory, frames);
        Ok(())
    })
    .unwrap_or_else(|err| match err {
        KernelError::InvalidArgument
        | KernelError::MachSendInvalidDest
        | KernelError::Terminated => Err(SamplingError::ThreadTerminated(
            "thread_suspend in with_suspended_thread",
            err,
        )),
        err => Err(SamplingError::Ignorable(
            "thread_suspend in with_suspended_thread",
            err,
        )),
    })?;

    if fold_recursive_prefix && !frames.is_empty() {
        let last_frame = *frames.last().unwrap();
        while frames.len() >= 2 && frames[frames.len() - 2] == last_frame {
            frames.pop();
        }
    }

    Ok(())
}

/// `frames` must be empty initially.
///
/// On return, `frames` will have the stack frames from callee-most to root-most.
fn do_stackwalk(
    stackwalker: StackwalkerRef,
    pc: u64,
    regs: UnwindRegsNative,
    memory: &mut ForeignMemory,
    frames: &mut Vec<FrameAddress>,
) {
    let mut read_stack = |addr| {
        if addr % 8 != 0 {
            // Unaligned address
            return Err(());
        }
        memory.read_u64_at_address(addr).map_err(|_| ())
    };

    // println!("begin unwinding for pc 0x{:x} with regs {:?}", pc, regs);

    use framehop::Unwinder;
    let mut iter = stackwalker
        .unwinder
        .iter_frames(pc, regs, stackwalker.cache, &mut read_stack);
    while let Ok(Some(address)) = iter.next() {
        frames.push(address);

        if frames.len() >= 10000 {
            break;
        }
    }
}

#[derive(Debug)]
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
        let number: &u64 = unsafe { self.get_type_ref_at_address(address) }?;
        Ok(*number)
    }

    fn data_index_for_address(&self, address: u64) -> std::result::Result<usize, usize> {
        self.data.binary_search_by(|d| {
            if d.address_range.start > address {
                Ordering::Greater
            } else if d.address_range.end <= address {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        })
    }

    fn get_data_for_range(
        &mut self,
        address_range: std::ops::Range<u64>,
    ) -> kernel_error::Result<&VmData> {
        let first_byte_addr = address_range.start;
        let last_byte_addr = address_range.end - 1;
        let vm_data = match (
            self.data_index_for_address(first_byte_addr),
            self.data_index_for_address(last_byte_addr),
        ) {
            (Ok(i), Ok(j)) if i == j => &self.data[i],
            (Ok(i), Ok(j)) | (Ok(i), Err(j)) | (Err(i), Ok(j)) | (Err(i), Err(j)) => {
                let start_addr = unsafe { mach_vm_trunc_page(first_byte_addr) };
                let end_addr = unsafe { mach_vm_trunc_page(last_byte_addr) + vm_page_size as u64 };
                let size = end_addr - start_addr;
                let data = VmData::map_from_task(self.task, start_addr, size)?;
                // Replace everything between i and j with the new combined range.
                self.data.splice(i..j, std::iter::once(data));
                &self.data[i]
            }
        };
        Ok(vm_data)
    }

    pub fn get_slice(&mut self, range: std::ops::Range<u64>) -> kernel_error::Result<&[u8]> {
        let vm_data = self.get_data_for_range(range.clone())?;
        Ok(vm_data.get_slice(range))
    }

    pub unsafe fn get_type_ref_at_address<T>(&mut self, address: u64) -> kernel_error::Result<&T> {
        let end_addr = address + mem::size_of::<T>() as u64;
        let vm_data = self.get_data_for_range(address..end_addr)?;
        Ok(vm_data.get_type_ref(address))
    }
}

#[derive(Debug)]
pub struct VmSubData {
    page_aligned_data: VmData,
    address_range: std::ops::Range<u64>,
}

impl VmSubData {
    /// address and size can be unaligned
    pub fn map_from_task(task: mach_port_t, address: u64, size: u64) -> kernel_error::Result<Self> {
        let last_byte = address + size - 1;
        let aligned_start_addr = unsafe { mach_vm_trunc_page(address) };
        let aligned_end_addr = unsafe { mach_vm_trunc_page(last_byte) + vm_page_size as u64 };

        Ok(Self {
            page_aligned_data: VmData::map_from_task(
                task,
                aligned_start_addr,
                aligned_end_addr - aligned_start_addr,
            )?,
            address_range: address..(address + size),
        })
    }

    pub fn get_full_slice(&self) -> &[u8] {
        self.page_aligned_data.get_slice(self.address_range.clone())
    }
}

impl Deref for VmSubData {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.get_full_slice()
    }
}

#[derive(Debug)]
pub struct VmData {
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
        let mut data_size: u32 = 0;
        unsafe {
            mach_vm_read(
                task,
                original_address,
                size,
                &mut data as *mut *mut u8 as *mut usize,
                &mut data_size as *mut u32,
            )
        }
        .into_result()?;

        Ok(Self {
            address_range: original_address..(original_address + data_size as u64),
            data,
            data_size: data_size as usize,
        })
    }

    /// original_address and size must be aligned to the page size.
    pub fn map_from_task(
        task: mach_port_t,
        original_address: u64,
        size: u64,
    ) -> kernel_error::Result<Self> {
        let aligned_addr = unsafe { mach_vm_trunc_page(original_address) };
        if aligned_addr != original_address {
            return Err(KernelError::InvalidAddress); // TODO: custom "unaligned address" error
        }
        if size % (unsafe { vm_page_size } as u64) != 0 {
            return Err(KernelError::InvalidValue); // TODO: custom "unaligned size" error
        }

        let mut data: *mut u8 = ptr::null_mut();
        let mut cur_protection: vm_prot_t = VM_PROT_NONE;
        let mut max_protection: vm_prot_t = VM_PROT_NONE;
        unsafe {
            mach_vm_remap(
                mach_task_self(),
                &mut data as *mut *mut u8 as *mut u64,
                size,
                0,
                1, /* anywhere: true */
                task,
                original_address,
                0,
                &mut cur_protection as *mut i32,
                &mut max_protection as *mut i32,
                VM_INHERIT_SHARE,
            )
        }
        .into_result()?;

        if cur_protection & VM_PROT_READ == 0 {
            // The mapped pages are not readable. Unmap them and return an error.
            let _ = unsafe { mach_vm_deallocate(mach_task_self(), data as _, size) };
            return Err(KernelError::NoAccess);
        }

        Ok(Self {
            address_range: original_address..(original_address + size),
            data,
            data_size: size as usize,
        })
    }

    pub fn get_slice(&self, address_range: std::ops::Range<u64>) -> &[u8] {
        assert!(self.address_range.start <= address_range.start);
        assert!(self.address_range.end >= address_range.end);
        let offset = address_range.start - self.address_range.start;
        let len = address_range.end - address_range.start;
        unsafe { std::slice::from_raw_parts(self.data.offset(offset as isize), len as usize) }
    }

    pub fn get_full_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.data, self.data_size) }
    }

    pub unsafe fn get_type_ref<T>(&self, address: u64) -> &T {
        assert!(address % mem::align_of::<T>() as u64 == 0);
        let range = address..(address + mem::size_of::<T>() as u64);
        let slice = self.get_slice(range);
        assert!(slice.len() == mem::size_of::<T>());
        &*(slice.as_ptr() as *const T)
    }
}

// Safety: Not sure actually.
unsafe impl Sync for VmData {}

impl Deref for VmData {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.get_full_slice()
    }
}

impl Drop for VmData {
    fn drop(&mut self) {
        let _ =
            unsafe { mach_vm_deallocate(mach_task_self(), self.data as _, self.data_size as _) };
    }
}

unsafe impl Send for VmData {}

#[derive(Clone, Debug)]
pub enum Modification<T> {
    Added(T),
    Removed(T),
}

pub fn diff_sorted_slices<T>(
    left: &[T],
    right: &[T],
    cmp_fn: impl Fn(&T, &T) -> Ordering,
) -> Vec<Modification<T>>
where
    T: Clone + Eq,
{
    let mut left_iter = left.iter();
    let mut right_iter = right.iter();
    let mut cur_left_elem = left_iter.next();
    let mut cur_right_elem = right_iter.next();
    let mut modifications = Vec::new();
    loop {
        match (cur_left_elem, cur_right_elem) {
            (None, None) => break,
            (Some(left), None) => {
                modifications.push(Modification::Removed(left.clone()));
                cur_left_elem = left_iter.next();
            }
            (None, Some(right)) => {
                modifications.push(Modification::Added(right.clone()));
                cur_right_elem = right_iter.next();
            }
            (Some(left), Some(right)) => match cmp_fn(left, right) {
                Ordering::Less => {
                    modifications.push(Modification::Removed(left.clone()));
                    cur_left_elem = left_iter.next();
                }
                Ordering::Greater => {
                    modifications.push(Modification::Added(right.clone()));
                    cur_right_elem = right_iter.next();
                }
                Ordering::Equal => {
                    if left != right {
                        modifications.push(Modification::Removed(left.clone()));
                        modifications.push(Modification::Added(right.clone()));
                    }
                    cur_left_elem = left_iter.next();
                    cur_right_elem = right_iter.next();
                }
            },
        }
    }
    modifications
}
