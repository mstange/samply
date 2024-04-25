use super::demangle_ocaml;
use msvc_demangler::DemangleFlags;

/// Attempt to demangle the passed-in string. This tries a bunch of different demangling schemes.
pub fn demangle_any(name: &str) -> String {
    if name.starts_with('?') {
        let flags = DemangleFlags::NO_ACCESS_SPECIFIERS
            | DemangleFlags::NO_FUNCTION_RETURNS
            | DemangleFlags::NO_MEMBER_TYPE
            | DemangleFlags::NO_MS_KEYWORDS
            | DemangleFlags::NO_THISTYPE
            | DemangleFlags::NO_CLASS_TYPE
            | DemangleFlags::SPACE_AFTER_COMMA
            | DemangleFlags::HUG_TYPE;
        return msvc_demangler::demangle(name, flags).unwrap_or_else(|_| name.to_string());
    }

    if name.starts_with("__S") {
        if let Ok(symbol) = scala_native_demangle::demangle_with_defaults(&name[1..name.len()]) {
            return symbol;
        }
    }

    if let Ok(demangled_symbol) = rustc_demangle::try_demangle(name) {
        return format!("{demangled_symbol:#}");
    }

    if name.starts_with('_') {
        let options = cpp_demangle::DemangleOptions::default().no_return_type();
        if let Ok(symbol) = cpp_demangle::Symbol::new(name) {
            if let Ok(demangled_string) = symbol.demangle(&options) {
                return demangled_string;
            }
        }
    }

    if let Some(symbol) = demangle_ocaml::demangle(name) {
        return symbol;
    }

    if name.starts_with('_') {
        return name.split_at(1).1.to_owned();
    }

    name.to_owned()
}

#[cfg(test)]
mod tests {
    use crate::demangle::demangle_any;
    #[test]
    fn cpp_demangling() {
        assert_eq!(
            demangle_any("_ZNK8KxVectorI16KxfArcFileRecordjEixEj"),
            "KxVector<KxfArcFileRecord, unsigned int>::operator[](unsigned int) const"
        )
    }

    #[test]
    fn mscvc_demangling() {
        assert_eq!(
            demangle_any("??_R3?$KxSet@V?$KxSpe@DI@@I@@8"),
            "KxSet<KxSpe<char, unsigned int>, unsigned int>::`RTTI Class Hierarchy Descriptor'"
        )
    }

    #[test]
    fn rust_demangling() {
        assert_eq!(
            demangle_any(
                "_RNvMsr_NtCs3ssYzQotkvD_3std4pathNtB5_7PathBuf3newCs15kBYyAo9fc_7mycrate"
            ),
            "<std::path::PathBuf>::new"
        )
    }

    #[test]
    fn ocaml_demangling() {
        assert_eq!(demangle_any("camlA__b__c_1002"), "A.b.c_1002")
    }

    #[test]
    fn scala_native_demangling() {
        assert_eq!(
            demangle_any("__SM17java.lang.IntegerD7compareiiiEo"),
            "java.lang.Integer.compare(Int,Int): Int"
        )
    }

    #[test]
    fn no_demangling() {
        assert_eq!(demangle_any("_!!!!!!!bla"), "!!!!!!!bla")
    }
}
