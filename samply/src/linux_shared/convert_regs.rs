use framehop::aarch64::UnwindRegsAarch64;
use framehop::x86_64::UnwindRegsX86_64;

use linux_perf_data::linux_perf_event_reader;

use linux_perf_event_reader::constants::{
    PERF_REG_ARM64_LR, PERF_REG_ARM64_PC, PERF_REG_ARM64_SP, PERF_REG_ARM64_X29, PERF_REG_X86_BP,
    PERF_REG_X86_IP, PERF_REG_X86_SP,
};
use linux_perf_event_reader::Regs;

pub use super::event_interpretation::EventInterpretation;

pub trait ConvertRegs {
    type UnwindRegs;
    fn convert_regs(regs: &Regs) -> (u64, u64, Self::UnwindRegs);
    fn regs_mask() -> u64;
}

pub struct ConvertRegsX86_64;
impl ConvertRegs for ConvertRegsX86_64 {
    type UnwindRegs = UnwindRegsX86_64;
    fn convert_regs(regs: &Regs) -> (u64, u64, UnwindRegsX86_64) {
        let ip = regs.get(PERF_REG_X86_IP).unwrap();
        let sp = regs.get(PERF_REG_X86_SP).unwrap();
        let bp = regs.get(PERF_REG_X86_BP).unwrap();
        let regs = UnwindRegsX86_64::new(ip, sp, bp);
        (ip, sp, regs)
    }

    fn regs_mask() -> u64 {
        1 << PERF_REG_X86_IP | 1 << PERF_REG_X86_SP | 1 << PERF_REG_X86_BP
    }
}

pub struct ConvertRegsAarch64;
impl ConvertRegs for ConvertRegsAarch64 {
    type UnwindRegs = UnwindRegsAarch64;
    fn convert_regs(regs: &Regs) -> (u64, u64, UnwindRegsAarch64) {
        let ip = regs.get(PERF_REG_ARM64_PC).unwrap();
        let lr = regs.get(PERF_REG_ARM64_LR).unwrap();
        let sp = regs.get(PERF_REG_ARM64_SP).unwrap();
        let fp = regs.get(PERF_REG_ARM64_X29).unwrap();
        let regs = UnwindRegsAarch64::new(lr, sp, fp);
        (ip, sp, regs)
    }

    fn regs_mask() -> u64 {
        1 << PERF_REG_ARM64_PC
            | 1 << PERF_REG_ARM64_LR
            | 1 << PERF_REG_ARM64_SP
            | 1 << PERF_REG_ARM64_X29
    }
}
