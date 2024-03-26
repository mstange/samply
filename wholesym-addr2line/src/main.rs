use std::borrow::Cow;
use std::io::{BufRead, Lines, StdinLock, Write};
use std::path::{Path, PathBuf};

use clap::parser::ValuesRef;
use clap::{value_parser, Arg, ArgAction, Command};

fn parse_uint_from_hex_string(string: &str) -> u32 {
    if string.len() > 2 && string.starts_with("0x") {
        u32::from_str_radix(&string[2..], 16).expect("Failed to parse address")
    } else {
        u32::from_str_radix(string, 16).expect("Failed to parse address")
    }
}

enum Addrs<'a> {
    Args(ValuesRef<'a, String>),
    Stdin(Lines<StdinLock<'a>>),
}

impl<'a> Iterator for Addrs<'a> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        let text = match *self {
            Addrs::Args(ref mut vals) => vals.next().map(Cow::from),
            Addrs::Stdin(ref mut lines) => lines.next().map(Result::unwrap).map(Cow::from),
        };
        text.as_ref()
            .map(Cow::as_ref)
            .map(parse_uint_from_hex_string)
    }
}

fn print_loc(
    file: &Option<wholesym::SourceFilePath>,
    line: Option<u32>,
    basenames: bool,
    llvm: bool,
) {
    if let Some(file) = file {
        let file = file.display_path();
        let path = if basenames {
            Path::new(&file).file_name().unwrap().to_string_lossy()
        } else {
            file.into()
        };
        print!("{path}:");
        if llvm {
            print!("{}:0", line.unwrap_or(0));
        } else if let Some(line) = line {
            print!("{}", line);
        } else {
            print!("?");
        }
        println!();
    } else if llvm {
        println!("??:0:0");
    } else {
        println!("??:?");
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let matches = Command::new("pdb-addr2line")
        .version("0.1")
        .about("A fast addr2line port for PDBs")
        .args(&[
            Arg::new("exe")
                .short('e')
                .long("exe")
                .value_name("filename")
                .value_parser(value_parser!(PathBuf))
                .help(
                    "Specify the name of the executable for which addresses should be translated.",
                )
                .required(true),
            Arg::new("sup")
                .long("sup")
                .value_name("filename")
                .help("Path to supplementary object file."),
            Arg::new("functions")
                .action(ArgAction::SetTrue)
                .short('f')
                .long("functions")
                .help("Display function names as well as file and line number information."),
            Arg::new("pretty")
                .action(ArgAction::SetTrue)
                .short('p')
                .long("pretty-print")
                .help(
                    "Make the output more human friendly: each location are printed on \
                     one line.",
                ),
            Arg::new("inlines")
                .action(ArgAction::SetTrue)
                .short('i')
                .long("inlines")
                .help(
                    "If the address belongs to a function that was inlined, the source \
             information for all enclosing scopes back to the first non-inlined \
             function will also be printed.",
                ),
            Arg::new("addresses")
                .action(ArgAction::SetTrue)
                .short('a')
                .long("addresses")
                .help(
                    "Display the address before the function name, file and line \
                     number information.",
                ),
            Arg::new("basenames")
                .action(ArgAction::SetTrue)
                .short('s')
                .long("basenames")
                .help("Display only the base of each file name."),
            Arg::new("demangle")
                .action(ArgAction::SetTrue)
                .short('C')
                .long("demangle")
                .help(
                    "Demangle function names. \
             Specifying a specific demangling style (like GNU addr2line) \
             is not supported. (TODO)",
                ),
            Arg::new("llvm")
                .action(ArgAction::SetTrue)
                .long("llvm")
                .help("Display output in the same format as llvm-symbolizer."),
            Arg::new("addrs")
                .action(ArgAction::Append)
                .help("Addresses to use instead of reading from stdin."),
        ])
        .get_matches();

    let do_functions = matches.get_flag("functions");
    let do_inlines = matches.get_flag("inlines");
    let pretty = matches.get_flag("pretty");
    let print_addrs = matches.get_flag("addresses");
    let basenames = matches.get_flag("basenames");
    let _demangle = matches.get_flag("demangle");
    let llvm = matches.get_flag("llvm");
    let path = matches.get_one::<PathBuf>("exe").unwrap();

    let config = wholesym::SymbolManagerConfig::new()
        .use_spotlight(true)
        .verbose(true)
        .respect_nt_symbol_path(true);
    let symbol_manager = wholesym::SymbolManager::with_config(config);
    let symbol_map = symbol_manager
        .load_symbol_map_for_binary_at_path(path, None)
        .await?;
    let symbol_file_origin = symbol_map.symbol_file_origin();

    let stdin = std::io::stdin();
    let addrs = matches
        .get_many("addrs")
        .map(Addrs::Args)
        .unwrap_or_else(|| Addrs::Stdin(stdin.lock().lines()));

    for probe in addrs {
        if print_addrs {
            if llvm {
                print!("0x{:x}", probe);
            } else {
                print!("0x{:016x}", probe);
            }
            if pretty {
                print!(": ");
            } else {
                println!();
            }
        }

        let mut printed_anything = false;
        if let Some(address_info) = symbol_map.lookup_relative_address(probe) {
            let frames = match address_info.frames {
                wholesym::FramesLookupResult::Available(frames) => Some(frames),
                wholesym::FramesLookupResult::NeedDwo { svma, .. } => {
                    symbol_map.lookup_ext(svma, &symbol_manager).await
                }
                wholesym::FramesLookupResult::External(external) => {
                    symbol_manager
                        .lookup_external(&symbol_file_origin, &external)
                        .await
                }
                wholesym::FramesLookupResult::Unavailable => None,
            };

            if let Some(frames) = frames {
                if do_functions || do_inlines {
                    for (i, frame) in frames.iter().enumerate() {
                        if pretty && i != 0 {
                            print!(" (inlined by) ");
                        }

                        if do_functions {
                            if let Some(func) = &frame.function {
                                print!("{func}");
                            } else if i == 0 {
                                print!("{}", address_info.symbol.name);
                            } else {
                                print!("??");
                            }

                            if pretty {
                                print!(" at ");
                            } else {
                                println!();
                            }
                        }

                        print_loc(&frame.file_path, frame.line_number, basenames, llvm);

                        printed_anything = true;

                        if !do_inlines {
                            break;
                        }
                    }
                } else if let Some(frame) = frames.first() {
                    print_loc(&frame.file_path, frame.line_number, basenames, llvm);
                    printed_anything = true;
                }
            } else {
                // Have no frames, but have a symbol.
                if do_functions {
                    print!("{}", address_info.symbol.name);

                    if pretty {
                        print!(" at ");
                    } else {
                        println!();
                    }
                }

                if llvm {
                    println!("??:0:0");
                } else {
                    println!("??:?");
                }

                printed_anything = true;
            }
        }

        if !printed_anything {
            if do_functions {
                print!("??");

                if pretty {
                    print!(" at ");
                } else {
                    println!();
                }
            }

            if llvm {
                println!("??:0:0");
            } else {
                println!("??:?");
            }
        }

        if llvm {
            println!();
        }
        std::io::stdout().flush().unwrap();
    }

    Ok(())
}
