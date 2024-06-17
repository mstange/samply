mod pico;

pub struct PicoProps {
    pub elf: String,
    pub device: String,
    pub save_file: Option<String>,
    pub bootrom_elf: Option<String>,
    pub reset: bool,
}

pub(crate) use pico::record_pico;