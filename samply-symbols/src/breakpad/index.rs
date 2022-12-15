use debugid::{CodeId, DebugId};
use memchr::memchr;
use nom::bytes::complete::{tag, take_while};
use nom::character::complete::{hex_digit1, space1};
use nom::combinator::{cut, map_res, opt, rest};
use nom::error::{Error, ErrorKind, ParseError};
use nom::multi::separated_list1;
use nom::sequence::{terminated, tuple};
use nom::{Err, IResult};

use std::collections::HashMap;
use std::convert::TryFrom;
use std::fmt::Debug;
use std::{mem, str};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakpadIndex {
    pub name: String,
    pub debug_id: DebugId,
    pub arch: String,
    pub os: String,
    pub code_id: Option<CodeId>,
    pub symbol_addresses: Vec<u32>,
    pub symbol_offsets: Vec<BreakpadSymbolType>,
    pub files: HashMap<u32, BreakpadFileLine>,
    pub inline_origins: HashMap<u32, BreakpadInlineOriginLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BreakpadSymbolType {
    Public(BreakpadPublicSymbol),
    Func(BreakpadFuncSymbol),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BreakpadPublicSymbol {
    /// The file offset at which there is the string `PUBLIC ` at the start of the line
    pub file_offset: u64,
    /// The length of the line, excluding line break (`\r*\n`). PUBLIC symbols only occupy a single line.
    pub line_length: u32,
}

impl BreakpadPublicSymbol {
    pub fn parse<'a>(
        &self,
        input: &'a [u8],
    ) -> Result<BreakpadPublicSymbolInfo<'a>, BreakpadParseError> {
        let (_rest, (_address, name)) =
            public_line(input).map_err(|_| BreakpadParseError::ParsingPublic)?;
        Ok(BreakpadPublicSymbolInfo {
            name: str::from_utf8(name).map_err(|_| BreakpadParseError::BadUtf8)?,
        })
    }
}

/// Returns the first line, excluding trailing `\r*\n`.
///
/// Advances the input to just after `\n`.
fn read_line_and_advance<'a>(input: &mut &'a [u8]) -> &'a [u8] {
    let mut line = if let Some(line_break) = memchr(b'\n', input) {
        let line = &input[..line_break];
        *input = &input[(line_break + 1)..];
        line
    } else {
        let line = *input;
        *input = &[];
        line
    };
    while line.last() == Some(&b'\r') {
        line = &line[..(line.len() - 1)];
    }
    line
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BreakpadFuncSymbol {
    /// The file offset at which there is the string `FUNC ` at the start of the line
    pub file_offset: u64,
    /// The number of bytes in the file taken up by this `FUNC` block, including its line record lines.
    pub block_length: u32,
}

impl BreakpadFuncSymbol {
    pub fn parse<'a>(
        &self,
        mut input: &'a [u8],
    ) -> Result<BreakpadFuncSymbolInfo<'a>, BreakpadParseError> {
        let first_line = read_line_and_advance(&mut input);
        let (_rest, (_address, size, name)) =
            func_line(first_line).map_err(|_| BreakpadParseError::ParsingFunc)?;
        let mut inlinees = Vec::new();
        let mut lines = Vec::new();
        while !input.is_empty() {
            let line = read_line_and_advance(&mut input);
            if line.starts_with(b"INLINE ") {
                let (_rest, new_inlinees) =
                    inline_line(line).map_err(|_| BreakpadParseError::ParsingInline)?;
                inlinees.extend(new_inlinees);
            } else if let Ok((_rest, line_data)) = func_line_data(line) {
                lines.push(line_data);
            }
        }
        inlinees.sort_by_key(|inlinee| (inlinee.depth, inlinee.address));
        Ok(BreakpadFuncSymbolInfo {
            name: str::from_utf8(name).map_err(|_| BreakpadParseError::BadUtf8)?,
            size,
            lines,
            inlinees,
        })
    }
}

pub trait FileOrInlineOrigin {
    fn offset_and_length(&self) -> (u64, u32);
    fn parse(line: &[u8]) -> Result<&str, BreakpadParseError>;
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BreakpadFileLine {
    /// The file offset at which there is the string `FILE ` at the start of the line
    pub file_offset: u64,
    /// The length of the line, excluding line break (`\r*\n`). `FILE` symbols only occupy a single line.
    pub line_length: u32,
}

impl FileOrInlineOrigin for BreakpadFileLine {
    fn offset_and_length(&self) -> (u64, u32) {
        (self.file_offset, self.line_length)
    }
    fn parse(input: &[u8]) -> Result<&str, BreakpadParseError> {
        let (_rest, (_index, name)) =
            file_line(input).map_err(|_| BreakpadParseError::ParsingFile)?;
        str::from_utf8(name).map_err(|_| BreakpadParseError::BadUtf8)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BreakpadInlineOriginLine {
    /// The file offset at which there is the string `INLINE_ORIGIN ` at the start of the line
    pub file_offset: u64,
    /// The length of the line, excluding line break (`\r*\n`). `INLINE_ORIGIN` symbols only occupy a single line.
    pub line_length: u32,
}

impl FileOrInlineOrigin for BreakpadInlineOriginLine {
    fn offset_and_length(&self) -> (u64, u32) {
        (self.file_offset, self.line_length)
    }
    fn parse(input: &[u8]) -> Result<&str, BreakpadParseError> {
        let (_rest, (_index, name)) =
            inline_origin_line(input).map_err(|_| BreakpadParseError::ParsingFile)?;
        str::from_utf8(name).map_err(|_| BreakpadParseError::BadUtf8)
    }
}

#[derive(Debug, Clone, Default)]
pub struct BreakpadIndexParser {
    line_buffer: LineBuffer,
    inner: BreakpadIndexParserInner,
}

impl BreakpadIndexParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn consume(&mut self, chunk: &[u8]) {
        let inner = &mut self.inner;
        let line_buffer = &mut self.line_buffer;
        line_buffer.consume(chunk, |offset, line| inner.process_line(offset, line));
    }

    pub fn finish(mut self) -> Result<BreakpadIndex, BreakpadParseError> {
        let inner = &mut self.inner;
        let final_offset = self
            .line_buffer
            .finish(|offset, line| inner.process_line(offset, line));
        self.inner.finish(final_offset)
    }
}

#[derive(Debug, Clone, Default)]
struct BreakpadIndexParserInner {
    module_info: Option<(DebugId, String, String, String)>,
    code_id: Option<CodeId>,
    symbols: Vec<(u32, BreakpadSymbolType)>,
    files: HashMap<u32, BreakpadFileLine>,
    inline_origins: HashMap<u32, BreakpadInlineOriginLine>,
    pending_func_block: Option<(u32, u64)>,
}

impl BreakpadIndexParserInner {
    pub fn process_line(&mut self, file_offset: u64, line: &[u8]) {
        let mut input = line;
        while input.last() == Some(&b'\r') {
            input = &input[..(input.len() - 1)];
        }
        if self.module_info.is_none() {
            // Every file must start with a "MODULE " line.
            if let Ok((_r, (os, arch, debug_id, name))) = module_line(input) {
                self.module_info =
                    Some((debug_id, os.to_string(), arch.to_string(), name.to_string()));
            }
            return;
        }
        let line_length = input.len() as u32;
        if let Ok((_r, (index, _filename))) = file_line(input) {
            self.files.insert(
                index,
                BreakpadFileLine {
                    file_offset,
                    line_length,
                },
            );
        } else if let Ok((_r, (index, _inline_origin))) = inline_origin_line(input) {
            self.inline_origins.insert(
                index,
                BreakpadInlineOriginLine {
                    file_offset,
                    line_length,
                },
            );
        } else if let Ok((_r, (address, _name))) = public_line(input) {
            self.finish_pending_func_block(file_offset);
            self.symbols.push((
                address,
                BreakpadSymbolType::Public(BreakpadPublicSymbol {
                    file_offset,
                    line_length,
                }),
            ));
        } else if let Ok((_r, (address, _size, _name))) = func_line(input) {
            self.finish_pending_func_block(file_offset);
            self.pending_func_block = Some((address, file_offset));
        } else if input.starts_with(b"STACK ")
            || input.starts_with(b"INFO ")
            || input.starts_with(b"STACK ")
        {
            self.finish_pending_func_block(file_offset);
        }
    }

    fn finish_pending_func_block(&mut self, non_func_line_start_offset: u64) {
        if let Some((address, file_offset)) = self.pending_func_block.take() {
            let block_length = (non_func_line_start_offset - file_offset) as u32;
            self.symbols.push((
                address,
                BreakpadSymbolType::Func(BreakpadFuncSymbol {
                    file_offset,
                    block_length,
                }),
            ));
        }
    }

    pub fn finish(mut self, file_end_offset: u64) -> Result<BreakpadIndex, BreakpadParseError> {
        self.finish_pending_func_block(file_end_offset);
        let BreakpadIndexParserInner {
            mut symbols,
            files,
            inline_origins,
            module_info,
            code_id,
            ..
        } = self;
        symbols.sort_by_key(|(address, _)| *address);
        symbols.dedup_by_key(|(address, _)| *address);
        let (symbol_addresses, symbol_offsets) = symbols.into_iter().unzip();

        let (debug_id, os, arch, name) =
            module_info.ok_or(BreakpadParseError::NoModuleInfoInSymFile)?;
        Ok(BreakpadIndex {
            debug_id,
            code_id,
            name,
            arch,
            os,
            symbol_addresses,
            symbol_offsets,
            files,
            inline_origins,
        })
    }
}

/// Consumes chunks and calls a callback for each line.
/// Leftover pieces are stored in a dynamically growing `Vec` in this object.
#[derive(Debug, Clone, Default)]
pub struct LineBuffer {
    leftover_bytes: Vec<u8>,
    /// The current offset in the file, taking into account all the bytes
    /// that have been consumed from the chunks. This also counts bytes that
    /// have been "consumed" by having been transferred to `leftover_bytes`.
    current_offset: u64,
}

impl LineBuffer {
    pub fn consume(&mut self, mut chunk: &[u8], mut f: impl FnMut(u64, &[u8])) {
        assert!(
            self.leftover_bytes.len() as u64 <= self.current_offset,
            "Caller supplied more self.leftover_bytes than we could have read ourselves"
        );
        loop {
            match memchr(b'\n', chunk) {
                None => {
                    self.leftover_bytes.extend_from_slice(chunk);
                    self.current_offset += chunk.len() as u64;
                    return;
                }
                Some(line_break_pos_in_chunk) => {
                    let chunk_until_line_break = &chunk[..line_break_pos_in_chunk];
                    // let chunk_until_line_break = (&chunk[..line_break_pos_in_chunk]).trim_end_matches(b'\r');
                    chunk = &chunk[(line_break_pos_in_chunk + 1)..];
                    let (line, line_start_offset) = if self.leftover_bytes.is_empty() {
                        (chunk_until_line_break, self.current_offset)
                    } else {
                        let line_start_offset =
                            self.current_offset - (self.leftover_bytes.len() as u64);
                        self.leftover_bytes.extend(chunk_until_line_break);
                        (self.leftover_bytes.as_slice(), line_start_offset)
                    };
                    self.current_offset += line_break_pos_in_chunk as u64 + 1;
                    f(line_start_offset, line);
                    self.leftover_bytes.clear();
                }
            };
        }
    }

    pub fn finish(self, mut f: impl FnMut(u64, &[u8])) -> u64 {
        if !self.leftover_bytes.is_empty() {
            let line_start_offset = self.current_offset - (self.leftover_bytes.len() as u64);
            f(line_start_offset, &self.leftover_bytes);
        }
        self.current_offset
    }
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum BreakpadParseError {
    #[error("Error parsing PUBLIC line")]
    ParsingPublic,

    #[error("Error parsing FILE line")]
    ParsingFile,

    #[error("Error parsing INLINE_ORIGIN line")]
    ParsingInlineOrigin,

    #[error("Error parsing FUNC line")]
    ParsingFunc,

    #[error("Error parsing INLINE line")]
    ParsingInline,

    #[error("Error parsing func line data line")]
    ParsingFuncLine,

    #[error("Malformed UTF-8")]
    BadUtf8,

    #[error("The Breakpad sym file did not start with a valid MODULE line")]
    NoModuleInfoInSymFile,
}

#[derive(Debug, Clone)]
pub struct BreakpadPublicSymbolInfo<'a> {
    pub name: &'a str,
}

#[derive(Debug, Clone)]
pub struct BreakpadFuncSymbolInfo<'a> {
    pub name: &'a str,
    pub size: u32,
    pub lines: Vec<SourceLine>,
    pub inlinees: Vec<Inlinee>,
}

impl<'a> BreakpadFuncSymbolInfo<'a> {
    /// Returns `(file_id, line, address)` of the line record that covers the
    /// given address. Line records describe locations at the deepest level of
    /// inlining at that address.
    ///
    /// For example, if we have an "inline call stack" A -> B -> C at this
    /// address, i.e. both the call to B and the call to C have been inlined all
    /// the way into A (A being the "outer function"), then this method reports
    /// locations in C.
    pub fn get_innermost_sourceloc(&self, addr: u32) -> Option<&SourceLine> {
        let line_index = match self.lines.binary_search_by_key(&addr, |line| line.address) {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        Some(&self.lines[line_index])
    }

    /// Returns `(call_file_id, call_line, address, inline_origin)` of the
    /// inlinee record that covers the given address at the given depth.
    ///
    /// We start at depth zero. For example, if we have an "inline call stack"
    /// A -> B -> C at an address, i.e. both the call to B and the call to C have
    /// been inlined all the way into A (A being the "outer function"), then the
    /// call A -> B is at level zero, and the call B -> C is at level one.
    pub fn get_inlinee_at_depth(&self, depth: u32, addr: u32) -> Option<&Inlinee> {
        let index = match self
            .inlinees
            .binary_search_by_key(&(depth, addr), |inlinee| (inlinee.depth, inlinee.address))
        {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        let inlinee = &self.inlinees[index];
        if inlinee.depth != depth {
            return None;
        }
        let end_address = inlinee.address.checked_add(inlinee.size)?;
        if addr < end_address {
            Some(inlinee)
        } else {
            None
        }
    }
}

/// Match a hex string, parse it to a u32 or a u64.
fn hex_str<T: std::ops::Shl<T, Output = T> + std::ops::BitOr<T, Output = T> + From<u8>>(
    input: &[u8],
) -> IResult<&[u8], T> {
    // Consume up to max_len digits. For u32 that's 8 digits and for u64 that's 16 digits.
    // Two hex digits form one byte.
    let max_len = mem::size_of::<T>() * 2;

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
        return Err(Err::Error(Error::from_error_kind(
            input,
            ErrorKind::HexDigit,
        )));
    }
    let remaining = &input[k..];
    Ok((remaining, res))
}

/// Match a decimal string, parse it to a u32.
///
/// This is doing everything manually so that we only look at each byte once.
/// With a naive implementation you might be looking at them three times: First
/// you might get a slice of acceptable characters from nom, then you might parse
/// that slice into a str (checking for utf-8 unnecessarily), and then you might
/// parse that string into a decimal number.
fn decimal_u32(input: &[u8]) -> IResult<&[u8], u32> {
    const MAX_LEN: usize = 10; // u32::MAX has 10 decimal digits
    let mut res: u64 = 0;
    let mut k = 0;
    for v in input.iter().take(MAX_LEN) {
        let digit = *v as char;
        let digit_value = match digit.to_digit(10) {
            Some(v) => v,
            None => break,
        };
        res = res * 10 + digit_value as u64;
        k += 1;
    }
    if k == 0 {
        return Err(Err::Error(Error::from_error_kind(input, ErrorKind::Digit)));
    }
    let res = u32::try_from(res)
        .map_err(|_| Err::Error(Error::from_error_kind(input, ErrorKind::TooLarge)))?;
    let remaining = &input[k..];
    Ok((remaining, res))
}

/// Take 0 or more non-space bytes.
fn non_space(input: &[u8]) -> IResult<&[u8], &[u8]> {
    take_while(|c: u8| c != b' ')(input)
}

// Matches a MODULE record.
fn module_line(input: &[u8]) -> IResult<&[u8], (&str, &str, DebugId, &str)> {
    let (input, _) = terminated(tag("MODULE"), space1)(input)?;
    let (input, (os, cpu, debug_id, name)) = cut(tuple((
        terminated(map_res(non_space, str::from_utf8), space1), // os
        terminated(map_res(non_space, str::from_utf8), space1), // cpu
        terminated(
            map_res(map_res(hex_digit1, str::from_utf8), DebugId::from_breakpad),
            space1,
        ), // debug id
        map_res(rest, str::from_utf8),                          // filename
    )))(input)?;
    Ok((input, (os, cpu, debug_id, name)))
}

// Matches a FILE record.
fn file_line(input: &[u8]) -> IResult<&[u8], (u32, &[u8])> {
    let (input, _) = terminated(tag("FILE"), space1)(input)?;
    let (input, (id, filename)) = cut(tuple((terminated(decimal_u32, space1), rest)))(input)?;
    Ok((input, (id, filename)))
}

// Matches an INLINE_ORIGIN record.
fn inline_origin_line(input: &[u8]) -> IResult<&[u8], (u32, &[u8])> {
    let (input, _) = terminated(tag("INLINE_ORIGIN"), space1)(input)?;
    let (input, (id, function)) = cut(tuple((terminated(decimal_u32, space1), rest)))(input)?;
    Ok((input, (id, function)))
}

// Matches a PUBLIC record.
fn public_line(input: &[u8]) -> IResult<&[u8], (u32, &[u8])> {
    let (input, _) = terminated(tag("PUBLIC"), space1)(input)?;
    let (input, (_multiple, address, _parameter_size, name)) = cut(tuple((
        opt(terminated(tag("m"), space1)),
        terminated(hex_str::<u64>, space1),
        terminated(hex_str::<u32>, space1),
        rest,
    )))(input)?;
    Ok((input, (address as u32, name)))
}

/// A mapping from machine code bytes to source line and file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLine {
    /// The start address relative to the module's load address.
    pub address: u32,
    /// The size of this range of instructions in bytes.
    pub size: u32,
    /// The source file name that generated this machine code.
    ///
    /// This is an index into `SymbolFile::files`.
    pub file: u32,
    /// The line number in `file` that generated this machine code.
    pub line: u32,
}

/// A single range which is covered by an inlined function call.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Inlinee {
    /// The depth of the inline call.
    pub depth: u32,
    /// The start address relative to the module's load address.
    pub address: u32,
    /// The size of this range of instructions in bytes.
    pub size: u32,
    /// The source file which contains the function call.
    ///
    /// This is an index into `SymbolFile::files`.
    pub call_file: u32,
    /// The line number in `call_file` for the function call.
    pub call_line: u32,
    /// The function name, as an index into `SymbolFile::inline_origins`.
    pub origin_id: u32,
}

// Matches line data after a FUNC record.
fn func_line_data(input: &[u8]) -> IResult<&[u8], SourceLine> {
    let (input, (address, size, line, file)) = tuple((
        terminated(hex_str::<u64>, space1),
        terminated(hex_str::<u32>, space1),
        terminated(decimal_u32, space1),
        decimal_u32,
    ))(input)?;
    Ok((
        input,
        SourceLine {
            address: address as u32,
            size,
            file,
            line,
        },
    ))
}

// Matches a FUNC record.
fn func_line(input: &[u8]) -> IResult<&[u8], (u32, u32, &[u8])> {
    let (input, _) = terminated(tag("FUNC"), space1)(input)?;
    let (input, (_multiple, address, size, _parameter_size, name)) = cut(tuple((
        opt(terminated(tag("m"), space1)),
        terminated(hex_str::<u32>, space1),
        terminated(hex_str::<u32>, space1),
        terminated(hex_str::<u32>, space1),
        rest,
    )))(input)?;
    Ok((input, (address, size, name)))
}

// Matches one entry of the form <address> <size> which is used at the end of an INLINE record
fn inline_address_range(input: &[u8]) -> IResult<&[u8], (u32, u32)> {
    tuple((terminated(hex_str::<u32>, space1), hex_str::<u32>))(input)
}

// Matches an INLINE record.
///
/// An INLINE record has the form `INLINE <inline_nest_level> <call_site_line> <call_site_file_id> <origin_id> [<address> <size>]+`.
fn inline_line(input: &[u8]) -> IResult<&[u8], impl Iterator<Item = Inlinee>> {
    let (input, _) = terminated(tag("INLINE"), space1)(input)?;
    let (input, (depth, call_line, call_file, origin_id)) = cut(tuple((
        terminated(decimal_u32, space1),
        terminated(decimal_u32, space1),
        terminated(decimal_u32, space1),
        terminated(decimal_u32, space1),
    )))(input)?;
    let (input, address_ranges) = cut(separated_list1(space1, inline_address_range))(input)?;
    Ok((
        input,
        address_ranges
            .into_iter()
            .map(move |(address, size)| Inlinee {
                address,
                size,
                call_file,
                call_line,
                depth,
                origin_id,
            }),
    ))
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn stuff() {
        let mut parser = BreakpadIndexParser::new();
        parser.consume(b"MODULE Linux x86_64 39CA3106713C8D0FFEE4605AFA2526670 libmozsandbox.so\nINFO CODE_ID ");
        parser.consume(b"0631CA393C710F8DFEE4605AFA2526671AD4EF17\nFILE 0 hg:hg.mozilla.org/mozilla-central:se");
        parser.consume(b"curity/sandbox/chromium/base/strings/safe_sprintf.cc:f150bc1f71d09e1e1941065951f0f5a3");
        parser.consume(b"8628f080");
        let index = parser.finish().unwrap();
        assert_eq!(
            index.files.get(&0).unwrap(),
            &BreakpadFileLine {
                file_offset: 125,
                line_length: 136,
            }
        );
        assert_eq!(
            index.debug_id,
            DebugId::from_breakpad("39CA3106713C8D0FFEE4605AFA2526670").unwrap()
        );
    }

    #[test]
    fn func_parsing() {
        let block =
            b"JUNK\nFUNC 1130 28 0 main\n1130 f 24 0\n113f 7 25 0\n1146 9 26 0\n114f 9 27 0\nJUNK";
        let func = BreakpadFuncSymbol {
            file_offset: "JUNK\n".len() as u64,
            block_length: (block.len() - "JUNK\n".len() - "\nJUNK".len()) as u32,
        };
        let input = &block[func.file_offset as usize..][..func.block_length as usize];
        let func = func.parse(input).unwrap();
        assert_eq!(func.name, "main");
        assert_eq!(func.size, 0x28);
        assert_eq!(func.lines.len(), 4);
        assert_eq!(
            func.lines[0],
            SourceLine {
                address: 0x1130,
                size: 0xf,
                file: 0,
                line: 24,
            }
        );
        assert_eq!(
            func.lines[3],
            SourceLine {
                address: 0x114f,
                size: 0x9,
                file: 0,
                line: 27,
            }
        );
    }
}
