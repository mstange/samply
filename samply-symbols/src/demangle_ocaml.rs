pub fn demangle(name: &str) -> Option<String> {
    if let Some(name) = name.strip_prefix("caml") {
        if name.chars().next().map_or(false, |c| !c.is_uppercase()) {
            return None;
        }

        let mut res = String::with_capacity(name.len());
        let mut chars = name.chars();

        while let Some(c) = chars.next() {
            let rest = chars.as_str();
            if c == '_' && rest.starts_with('_') {
                chars.next();
                res.push('.');
            } else if c == '$' && rest.len() >= 2 {
                if let Ok(c) = u8::from_str_radix(&rest[..2], 16) {
                    chars.next();
                    chars.next();
                    res.push(c as char);
                }
            } else {
                res.push(c);
            }
        }

        return Some(res);
    }

    None
}

#[cfg(test)]
mod test {
    use super::demangle;

    #[test]
    fn demangle_ocaml() {
        assert!(demangle("main") == None);
        assert!(demangle("camlStdlib__array__map_154") == Some("Stdlib.array.map_154".to_string()));
        assert!(
            demangle("camlStdlib__anon_fn$5bstdlib$2eml$3a334$2c0$2d$2d54$5d_1453")
                == Some("Stdlib.anon_fn[stdlib.ml:334,0--54]_1453".to_string())
        );
        assert!(
            demangle("camlStdlib__bytes__$2b$2b_2205") == Some("Stdlib.bytes.++_2205".to_string())
        );
        assert!(demangle("camlFoo$ff") == Some("Foo\u{ff}".to_string()));
        assert!(demangle("camlFoo_") == Some("Foo_".to_string()));
        assert!(demangle("camlFoo__") == Some("Foo.".to_string()));
        assert!(demangle("camlFoo$") == Some("Foo$".to_string()));
        assert!(demangle("camlFoo$a") == Some("Foo$a".to_string()));
        assert!(demangle("camlFoo$$") == Some("Foo$$".to_string()));
    }
}
