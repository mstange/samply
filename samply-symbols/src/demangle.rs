use super::demangle_ocaml;
use msvc_demangler::DemangleFlags;

/// MangleKind represents the possible name mangling that we may meet when trying to demangle a name.
/// In some situations, it is quite useful to know *how* a name was mangled, even after we have decoded it.
#[derive(Debug, PartialEq)]
pub enum MangleKind {
    MSVC,
    Rust,
    OCaml,
    Itanium,
    Unknown
}

pub fn demangle_any(name: &str) -> (MangleKind, String) {
    if name.starts_with('?') {
        let flags = DemangleFlags::NO_ACCESS_SPECIFIERS
            | DemangleFlags::NO_FUNCTION_RETURNS
            | DemangleFlags::NO_MEMBER_TYPE
            | DemangleFlags::NO_MS_KEYWORDS
            | DemangleFlags::NO_THISTYPE
            | DemangleFlags::NO_CLASS_TYPE
            | DemangleFlags::SPACE_AFTER_COMMA
            | DemangleFlags::HUG_TYPE;
        return (MangleKind::MSVC, msvc_demangler::demangle(name, flags).unwrap_or_else(|_| name.to_string()));
    }

    if let Ok(demangled_symbol) = rustc_demangle::try_demangle(name) {
        return (MangleKind::Rust, format!("{demangled_symbol:#}"));
    }

    if name.starts_with('_') {
        let options = cpp_demangle::DemangleOptions::default().no_return_type();
        if let Ok(symbol) = cpp_demangle::Symbol::new(name) {
            if let Ok(demangled_string) = symbol.demangle(&options) {
                return (MangleKind::Itanium, demangled_string);
            }
        }
    }

    if let Some(symbol) = demangle_ocaml::demangle(name) {
        return (MangleKind::OCaml, symbol);
    }

    if name.starts_with('_') {
        return (MangleKind::Itanium, name.split_at(1).1.to_owned());
    }

    (MangleKind::Unknown, name.to_owned())
}

