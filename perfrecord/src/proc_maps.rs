use libc::strlen;
use mach;
use mach::kern_return::{kern_return_t, KERN_SUCCESS};
use mach::message::mach_msg_type_number_t;
use mach::port::mach_port_t;
use mach::vm_types::{mach_vm_address_t, mach_vm_size_t};
use std;
use std::io;
use uuid::Uuid;

use crate::dyld_bindings;
use dyld_bindings::{
    dyld_all_image_infos, dyld_image_info, load_command, mach_header_64, segment_command_64,
    uuid_command,
};

#[derive(Debug, Clone)]
pub struct DyldInfo {
    pub file: String,
    pub address: u64,
    pub vmsize: u64,
    pub uuid: Option<Uuid>,
}

/// Returns basic information on modules loaded up by dyld. This lets
/// us get the filename/address of the system Ruby or Python frameworks for instance.
/// (which won't appear as a separate entry in vm_regions returned by get_process_maps)
pub fn get_dyld_info(task: mach_port_t) -> io::Result<Vec<DyldInfo>> {
    // Adapted from :
    // https://stackoverflow.com/questions/4309117/determining-programmatically-what-modules-are-loaded-in-another-process-os-x
    // https://blog.lse.epita.fr/articles/82-playing-with-mach-os-and-dyld.html

    // This gets addresses to TEXT sections ... but we really want addresses to DATA
    // this is a good start though
    // hmm
    use mach::task::task_info;
    use mach::task_info::{task_info_t, TASK_DYLD_INFO};

    let mut vec = Vec::new();

    // Note: this seems to require osx MAC_OS_X_VERSION_10_6 or greater
    // https://chromium.googlesource.com/breakpad/breakpad/+/master/src/client/mac/handler/dynamic_images.cc#388
    let mut dyld_info = task_dyld_info {
        all_image_info_addr: 0,
        all_image_info_size: 0,
        all_image_info_format: 0,
    };

    // TASK_DYLD_INFO_COUNT is #define'd to be 5 in /usr/include/mach/task_info.h
    // ... doesn't seem to be included in the mach crate =(
    let mut count: mach_msg_type_number_t = 5;
    unsafe {
        if task_info(
            task,
            TASK_DYLD_INFO,
            &mut dyld_info as *mut task_dyld_info as task_info_t,
            &mut count,
        ) != KERN_SUCCESS
        {
            return Err(io::Error::last_os_error());
        }
    }

    // Read in the dyld_all_image_infos information here here.
    let mut image_infos = dyld_all_image_infos::default();
    let mut read_len = std::mem::size_of_val(&image_infos) as mach_vm_size_t;

    let result = unsafe {
        // While we could use the read_process_memory crate for this, this adds a dependency
        // for something that is pretty trivial
        vm_read_overwrite(
            task,
            dyld_info.all_image_info_addr,
            read_len,
            (&mut image_infos) as *mut dyld_all_image_infos as mach_vm_address_t,
            &mut read_len,
        )
    };
    if result != KERN_SUCCESS {
        return Err(io::Error::last_os_error());
    }

    // copy the infoArray element of dyld_all_image_infos ovber
    let mut modules = vec![dyld_image_info::default(); image_infos.infoArrayCount as usize];
    let mut read_len = (std::mem::size_of::<dyld_image_info>()
        * image_infos.infoArrayCount as usize) as mach_vm_size_t;
    let result = unsafe {
        vm_read_overwrite(
            task,
            image_infos.infoArray as mach_vm_address_t,
            read_len,
            modules.as_mut_ptr() as mach_vm_address_t,
            &mut read_len,
        )
    };
    if result != KERN_SUCCESS {
        return Err(io::Error::last_os_error());
    }

    for module in modules {
        let mut read_len = 512 as mach_vm_size_t;
        let mut image_filename = [0_i8; 512];
        let result = unsafe {
            vm_read_overwrite(
                task,
                module.imageFilePath as mach_vm_address_t,
                read_len,
                image_filename.as_mut_ptr() as mach_vm_address_t,
                &mut read_len,
            )
        };
        if result != KERN_SUCCESS {
            return Err(io::Error::last_os_error());
        }

        let ptr = image_filename.as_ptr();
        let slice = unsafe { std::slice::from_raw_parts(ptr as *mut u8, strlen(ptr)) };
        let filename = std::str::from_utf8(slice).unwrap().to_owned();

        // read in the mach header
        let mut header = mach_header_64::default();
        let mut read_len = std::mem::size_of_val(&header) as mach_vm_size_t;
        let result = unsafe {
            // While we could use the read_process_memory crate for this, this adds a dependency
            // for something that is pretty trivial
            vm_read_overwrite(
                task,
                module.imageLoadAddress as u64,
                read_len,
                (&mut header) as *mut mach_header_64 as mach_vm_address_t,
                &mut read_len,
            )
        };
        if result != KERN_SUCCESS {
            return Err(io::Error::last_os_error());
        }

        let mut commands_buffer = vec![0_i8; header.sizeofcmds as usize];
        let mut read_len = mach_vm_size_t::from(header.sizeofcmds);
        let result = unsafe {
            vm_read_overwrite(
                task,
                (module.imageLoadAddress as usize + std::mem::size_of_val(&header))
                    as mach_vm_size_t,
                read_len,
                commands_buffer.as_mut_ptr() as mach_vm_address_t,
                &mut read_len,
            )
        };
        if result != KERN_SUCCESS {
            return Err(io::Error::last_os_error());
        }

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

extern "C" {
    fn vm_read_overwrite(
        target_task: mach_port_t,
        address: mach_vm_address_t,
        size: mach_vm_size_t,
        data: mach_vm_address_t,
        out_size: *mut mach_vm_size_t,
    ) -> kern_return_t;
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
