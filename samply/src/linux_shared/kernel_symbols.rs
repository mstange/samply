use std::path::Path;
use std::sync::Arc;
use std::{fmt::Debug, path::PathBuf};

use fxprof_processed_profile::{Symbol, SymbolTable};
use object::{elf, read, NativeEndian, Object};
use read::elf::NoteIterator;

use crate::shared::utils::open_file_with_fallback;

#[derive(Debug, thiserror::Error)]
pub enum KernelSymbolsError {
    #[error("Could not read /sys/kernel/notes: {0}")]
    CouldNotReadKernelNotes(#[source] std::io::Error),

    #[error("Did not find a NT_GNU_BUILD_ID note in /sys/kernel/notes")]
    CouldNotFindBuildIdNote,

    #[error("Could not read /proc/kallsyms: {0}")]
    CouldNotReadProcKallsyms(#[source] std::io::Error),

    #[error("Did not find a _text symbol in the kernel symbol list")]
    NoTextSymbol,

    #[error("Relative address {0:#x} does not fit into u32")]
    RelativeAddressTooLarge(u64),
}

#[derive(Debug, Clone)]
pub struct KernelSymbols {
    pub build_id: Vec<u8>,
    pub base_avma: u64,
    pub symbol_table: Arc<SymbolTable>,
}

impl KernelSymbols {
    pub fn new_for_running_kernel() -> Result<Self, KernelSymbolsError> {
        let notes = std::fs::read("/sys/kernel/notes")
            .map_err(KernelSymbolsError::CouldNotReadKernelNotes)?;
        let build_id = build_id_from_notes_section_data(&notes)
            .ok_or(KernelSymbolsError::CouldNotFindBuildIdNote)?
            .to_owned();
        let kallsyms = std::fs::read("/proc/kallsyms")
            .map_err(KernelSymbolsError::CouldNotReadProcKallsyms)?;
        let (base_avma, symbol_table) = parse_kallsyms(&kallsyms)?;
        let symbol_table = Arc::new(symbol_table);
        Ok(KernelSymbols {
            build_id,
            base_avma,
            symbol_table,
        })
    }
}

pub fn build_id_from_notes_section_data(section_data: &[u8]) -> Option<&[u8]> {
    let mut note_iter =
        NoteIterator::<elf::FileHeader64<NativeEndian>>::new(NativeEndian, 4, section_data).ok()?;
    while let Ok(Some(note)) = note_iter.next() {
        if note.name() == elf::ELF_NOTE_GNU && note.n_type(NativeEndian) == elf::NT_GNU_BUILD_ID {
            return Some(note.desc());
        }
    }
    None
}

struct KallSymIter<'a> {
    remaining_data: &'a [u8],
}

impl<'a> KallSymIter<'a> {
    pub fn new(proc_kallsyms: &'a [u8]) -> Self {
        Self {
            remaining_data: proc_kallsyms,
        }
    }
}

impl<'a> Iterator for KallSymIter<'a> {
    type Item = (u64, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining_data.is_empty() {
            return None;
        }

        // Format: <hex address> <space> <letter> <space> <name> \n
        let (after_address, address) = hex_str::<u64>(self.remaining_data).ok()?;
        let starting_with_name = after_address.get(3..)?; // Skip <space> <letter> <space>
        match memchr::memchr(b'\n', starting_with_name) {
            Some(name_len) => {
                self.remaining_data = &starting_with_name[(name_len + 1)..];
                Some((address, &starting_with_name[..name_len]))
            }
            None => {
                self.remaining_data = &[];
                Some((address, starting_with_name))
            }
        }
    }
}

pub fn parse_kallsyms(data: &[u8]) -> Result<(u64, SymbolTable), KernelSymbolsError> {
    let mut symbols = Vec::new();

    let mut text_addr = None;
    for (absolute_addr, symbol_name) in KallSymIter::new(data) {
        match (text_addr, symbol_name) {
            (None, b"_text") => {
                text_addr = Some(absolute_addr);
                symbols.push(Symbol {
                    address: 0,
                    size: None,
                    name: "_text".to_string(),
                });
            }
            (Some(text_addr), _) if absolute_addr >= text_addr => {
                let relative_address = absolute_addr - text_addr;
                let relative_address = u32::try_from(relative_address)
                    .map_err(|_| KernelSymbolsError::RelativeAddressTooLarge(relative_address))?;
                symbols.push(Symbol {
                    address: relative_address,
                    size: None,
                    name: String::from_utf8_lossy(symbol_name).to_string(),
                });
            }
            _ => {
                // Ignore symbols before the _text symbol.
            }
        }
    }
    let text_addr = text_addr.ok_or(KernelSymbolsError::NoTextSymbol)?;
    Ok((text_addr, SymbolTable::new(symbols)))
}

/// Match a hex string, parse it to a u32 or a u64.
fn hex_str<T: std::ops::Shl<T, Output = T> + std::ops::BitOr<T, Output = T> + From<u8>>(
    input: &[u8],
) -> Result<(&[u8], T), &'static str> {
    // Consume up to max_len digits. For u32 that's 8 digits and for u64 that's 16 digits.
    // Two hex digits form one byte.
    let max_len = std::mem::size_of::<T>() * 2;

    let mut res: T = T::from(0);
    let mut k = 0;
    for v in input.iter().take(max_len) {
        let digit = match (*v as char).to_digit(16) {
            Some(v) => v,
            None => break,
        };
        res = res << T::from(4);
        res = res | T::from(digit as u8);
        k += 1;
    }
    if k == 0 {
        return Err("Bad hex digit");
    }
    let remaining = &input[k..];
    Ok((remaining, res))
}

pub fn kernel_module_build_id(path: &Path, binary_lookup_dirs: &[PathBuf]) -> Option<Vec<u8>> {
    let file = open_file_with_fallback(path, binary_lookup_dirs).ok()?.0;
    let mmap = unsafe { memmap2::MmapOptions::new().map(&file) }.ok()?;
    let obj = object::File::parse(&mmap[..]).ok()?;
    match obj.build_id() {
        Ok(Some(build_id)) => Some(build_id.to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod test {
    use debugid::CodeId;

    use super::build_id_from_notes_section_data;
    use crate::linux_shared::kernel_symbols::parse_kallsyms;

    #[test]
    fn test() {
        let build_id = build_id_from_notes_section_data(b"\x04\0\0\0\x14\0\0\0\x03\0\0\0GNU\0\x98Kvo\x1c\xb5i\x9c;\x1bw\xb5\x92\x98<\"\xe9\xd1\x97\xad\x06\0\0\0\x04\0\0\0\x01\x01\0\0Linux\0\0\0\0\0\0\0\x06\0\0\0\x01\0\0\0\0\x01\0\0Linux\0\0\0\0\0\0\0");
        let code_id = CodeId::from_binary(build_id.unwrap());
        assert_eq!(code_id.as_str(), "984b766f1cb5699c3b1b77b592983c22e9d197ad");
    }

    #[test]
    fn test2() {
        let kallsyms = br#"ffff8000081e0000 T _text
ffff8000081f0000 t bcm2835_handle_irq
ffff8000081f0000 T _stext
ffff8000081f0000 T __irqentry_text_start
ffff8000081f0060 t bcm2836_arm_irqchip_handle_irq
ffff8000081f00e0 t dw_apb_ictl_handle_irq
ffff8000081f0190 t sun4i_handle_irq"#;
        let (base_avma, symbol_table) = parse_kallsyms(kallsyms).unwrap();
        assert_eq!(base_avma, 0xffff8000081e0000);
        assert_eq!(
            &symbol_table.lookup(0x10061).unwrap().name,
            "bcm2836_arm_irqchip_handle_irq"
        );
        assert_eq!(
            &symbol_table.lookup(0x10054).unwrap().name,
            "__irqentry_text_start"
        );
    }

    #[test]
    fn test3() {
        let kallsyms = br#"0000000000000000 A fixed_percpu_data
0000000000000000 A __per_cpu_start
0000000000001000 A cpu_debug_store
0000000000002000 A irq_stack_backing_store
0000000000006000 A cpu_tss_rw
0000000000032080 A steal_time
00000000000320c0 A apf_reason
0000000000033000 A __per_cpu_end
ffffffffa7e00000 T startup_64
ffffffffa7e00000 T _stext
ffffffffa7e00000 T _text
ffffffffa7e00040 T secondary_startup_64
ffffffffa7e00045 T secondary_startup_64_no_verify
ffffffffa7e00110 t verify_cpu
ffffffffa7e00210 T sev_verify_cbit"#;
        let (base_avma, symbol_table) = parse_kallsyms(kallsyms).unwrap();
        assert_eq!(base_avma, 0xffffffffa7e00000);
        assert_eq!(
            &symbol_table.lookup(0x61).unwrap().name,
            "secondary_startup_64_no_verify"
        );
    }

    #[test]
    fn test4() {
        // In this example, there are spots where the address goes backwards.
        // The kernel modules seem to be loaded before the regular vmlinux image.
        // For example, [tls] starts at ffff800001717000, which is before _text at ffff8000081e0000.
        let kallsyms = br#"ffff8000081e0000 T _text
ffff8000081f0000 t bcm2835_handle_irq
ffff8000081f0000 T _stext
ffff8000081f0000 T __irqentry_text_start
ffff8000081f0d28 T __softirqentry_text_end
ffff8000081f1000 T vectors
ffff8000081f1800 t __bad_stack
ffff80000869fd40 t __bpf_trace_iomap_readpage_class
ffff800008b78ad0 t tegra_clk_periph_fixed_is_enabled
ffff800008b78b54 t tegra_clk_periph_fixed_enable
ffff800008fdf910 T hv_is_hibernation_supported
ffff800008fdfa70 W hv_setup_kexec_handler
ffff800008fdfc10 T hv_common_cpu_die
ffff8000092cc76c t skip_pte
ffff8000092cc77c t __idmap_kpti_secondary
ffff8000092cc7c4 T __cpu_setup
ffff8000092cf0e0 T __sdei_asm_exit_trampoline
ffff8000092d0000 T __entry_tramp_text_end
ffff8000092e0000 D kimage_vaddr
ffff8000092e0000 D _etext
ffff8000092e0000 D __start_rodata
ffff8000092e0008 d __func__.10
ffff8000092e9040 d armv8_a53_perf_cache_map
ffff8000092e91e8 D arch_kgdb_ops
ffff8000092e93a0 D kexec_file_loaders
ffff800009445910 d acpi_thermal_pm
ffff800009a5ab18 d __tpstrtab_mptcp_subflow_get_send
ffff800009a5ab30 R __start_pci_fixups_early
ffff800009a5b250 R __end_pci_fixups_early
ffff800009a5d3b0 R __end_pci_fixups_suspend
ffff800009a5d3b0 R __start_pci_fixups_suspend_late
ffff800009a5d3c0 r __ksymtab_I_BDEV
ffff800009a5d3c0 R __end_builtin_fw
ffff800009a5d3c0 R __end_pci_fixups_suspend_late
ffff800009a5d3c0 R __start___ksymtab
ffff800009a5d3c0 R __start_builtin_fw
ffff800009a5d3cc r __ksymtab_LZ4_decompress_fast
ffff800009acb940 d __modver_attr
ffff800009acb940 D __start___modver
ffff800009acb940 R __stop___param
ffff800009acbc58 d __modver_attr
ffff800009acbca0 R __start___ex_table
ffff800009acbca0 D __stop___modver
ffff800009acda40 R __start_notes
ffff800009acda40 R __stop___ex_table
ffff800009acda64 r _note_53
ffff800009acda7c r _note_52
ffff800009acda94 R __start_BTF
ffff800009acda94 R __stop_notes
ffff80000a060553 R __stop_BTF
ffff80000a060554 r btf_seq_file_ids
ffff80000a060554 r __BTF_ID__struct__seq_file__663
ffff80000a060558 r bpf_task_pt_regs_ids
ffff80000a060558 r __BTF_ID__struct__pt_regs__668
ffff80000a06055c r btf_allowlist_d_path
ffff80000a060ab4 R btf_sock_ids
ffff80000a060ab4 r __BTF_ID__struct__inet_sock__1297
ffff80000a060b08 r bpf_tcp_ca_kfunc_ids
ffff80000a061000 D __end_rodata
ffff80000a061000 D __hyp_rodata_start
ffff80000a062000 D idmap_pg_dir
ffff80000a062000 D __hyp_rodata_end
ffff80000a065000 T idmap_pg_end
ffff80000a065000 T tramp_pg_dir
ffff80000a070000 T primary_entry
ffff80000a070000 T _sinittext
ffff80000a070000 T __init_begin
ffff80000a070000 T __inittext_begin
ffff80000a070020 t preserve_boot_args
ffff80000a070040 t __create_page_tables
ffff80000a070338 t __primary_switched
ffff80000a1048f8 t packet_exit
ffff80000a104940 t rfkill_exit
ffff80000a104980 T rfkill_handler_exit
ffff80000a1049b8 t exit_dns_resolver
ffff80000a104a08 R __alt_instructions
ffff80000a104a08 T __exittext_end
ffff80000a13fbec R __alt_instructions_end
ffff80000a140000 d xbc_namebuf
ffff80000a140000 D __initdata_begin
ffff80000ada1df8 d fib_rules_net_ops
ffff80000ada1e38 d fib_rules_notifier
ffff80000ada1fa8 d print_fmt_neigh__update
ffff80000add5f40 D __tracepoint_mm_vmscan_lru_shrink_active
ffff80000ae33008 D __mmuoff_data_end
ffff80000ae33200 R _edata
ffff80000ae34000 B __bss_start
ffff80000ae34000 B __hyp_bss_start
ffff80000af5dd9c b pm_nl_pernet_id
ffff80000af5dda0 b ___done.0
ffff80000af5dda1 B __bss_stop
ffff80000af5e000 B init_pg_dir
ffff80000af63000 B init_pg_end
ffff80000af70000 B _end
ffff800001717000 t $x  [tls]
ffff800001717000 t tls_get_info_size   [tls]
ffff8000017290c0 d $d  [tls]
ffff800001717020 t tls_update  [tls]
ffff800001411010 t choose_data_offset  [raid10]
ffff80000141f058 d $d  [raid10]
ffff800001411050 t __raid10_find_phys  [raid10]
ffff80000b543a4c t bpf_prog_6deef7357e7b4530   [bpf]
ffff80000b5c5744 t bpf_prog_654d7024997e7811   [bpf]"#;
        let (base_avma, symbol_table) = parse_kallsyms(kallsyms).unwrap();
        assert_eq!(base_avma, 0xffff8000081e0000);
        assert_eq!(
            &symbol_table.lookup(0x998b20).unwrap().name,
            "tegra_clk_periph_fixed_is_enabled"
        );
    }
}
