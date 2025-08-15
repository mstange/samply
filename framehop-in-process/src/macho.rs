use std::collections::HashMap;
use std::ffi::c_void;
use std::mem;
use std::ops::Range;
use std::sync::Arc;

use object::LittleEndian;
use object::macho::{MachHeader64, SegmentCommand64};
use object::read::macho::{MachHeader, Section, Segment};

use framehop::{ExplicitModuleSectionInfo, Module};

use super::module_data::{ModuleDataSlice, RawModuleData};

pub fn get_module_macho(
    base_ptr: *const c_void,
) -> Result<(Arc<RawModuleData>, Module<ModuleDataSlice>), ()> {
    let header_ptr = base_ptr as *const MachHeader64<LittleEndian>;
    let header = unsafe { &(*header_ptr) };

    let endian = LittleEndian;
    let header_and_command_data = unsafe {
        std::slice::from_raw_parts(
            base_ptr as *const u8,
            mem::size_of::<MachHeader64<LittleEndian>>() + header.sizeofcmds(endian) as usize,
        )
    };
    let mut load_commands = (*header)
        .load_commands(endian, header_and_command_data, 0)
        .map_err(|_| ())?;

    let base_avma = header_ptr as usize as u64;
    let mut base_svma = 0;
    let mut vmsize: u64 = 0;
    let mut sections = HashMap::new();

    while let Ok(Some(command)) = load_commands.next() {
        if let Ok(Some((segment, section_data))) = SegmentCommand64::from_command(command) {
            if segment.name() == b"__TEXT" {
                base_svma = segment.vmaddr(endian);
                vmsize = segment.vmsize(endian);

                for section in segment.sections(endian, section_data).map_err(|_| ())? {
                    let addr = section.addr.get(endian);
                    let size = section.size.get(endian);
                    sections.insert(section.name(), (addr, size));
                }
            }
        }
    }

    let section_svma_range = |name: &[u8]| -> Option<Range<u64>> {
        sections.get(name).map(|(addr, size)| *addr..*addr + *size)
    };

    let module_data = unsafe { RawModuleData::new(base_ptr as *const u8, vmsize as usize) };
    let module_data = Arc::new(module_data);

    let module_data_slice = |svma_range: Range<u64>| -> Option<ModuleDataSlice> {
        let start_byte_offset = svma_range.start.checked_sub(base_svma)? as usize;
        let byte_len = svma_range.end.checked_sub(svma_range.start)? as usize;
        module_data.slice(start_byte_offset, byte_len)
    };

    let eh_frame_svma = section_svma_range(b"__eh_frame");
    let eh_frame_hdr_svma = section_svma_range(b"__eh_frame_hdr");
    let unwind_info_svma = section_svma_range(b"__unwind_info");

    let section_info = ExplicitModuleSectionInfo {
        base_svma,
        text_svma: section_svma_range(b"__text"),
        stubs_svma: section_svma_range(b"__stubs"),
        stub_helper_svma: section_svma_range(b"__stub_helper"),
        got_svma: section_svma_range(b"__got"),
        eh_frame_svma: eh_frame_svma.clone(),
        eh_frame_hdr_svma: eh_frame_hdr_svma.clone(),
        text_segment_svma: Some(base_svma..base_svma + vmsize),

        text_segment: module_data.slice(0, vmsize as usize),
        unwind_info: unwind_info_svma.and_then(module_data_slice),
        eh_frame: eh_frame_svma.and_then(module_data_slice),
        eh_frame_hdr: eh_frame_hdr_svma.and_then(module_data_slice),
        debug_frame: None,
        text: None,
    };

    let module = Module::new(
        "SomeModule".to_string(),
        base_avma..base_avma + vmsize,
        base_avma,
        section_info,
    );

    Ok((module_data, module))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_binary() {
        let load_addr = 0x0000000100000000usize;
        let base_ptr = load_addr as *const c_void;
        match get_module_macho(base_ptr) {
            Ok(_) => {}
            Err(e) => {
                eprintln!("error {e:?}");
            }
        }
    }
}
