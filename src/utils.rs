use std::fmt;

pub struct HexValue(pub u64);
impl fmt::Debug for HexValue {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(fmt, "0x{:016X}", self.0)
    }
}

pub struct HexSlice<'a>(pub &'a [u64]);
impl<'a> fmt::Debug for HexSlice<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        fmt.debug_list()
            .entries(self.0.iter().map(|&value| HexValue(value)))
            .finish()
    }
}
