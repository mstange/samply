#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Region {
    pub start: u64,
    pub end: u64,
    pub is_read: bool,
    pub is_write: bool,
    pub is_executable: bool,
    pub is_shared: bool,
    pub file_offset: u64,
    pub major: u32,
    pub minor: u32,
    pub inode: u64,
    pub name: String,
}

fn get_until<'a>(p: &mut &'a str, delimiter: char) -> &'a str {
    let mut found = None;
    for (index, ch) in p.char_indices() {
        if ch == delimiter {
            found = Some(index);
            break;
        }
    }

    if let Some(index) = found {
        let (before, after) = p.split_at(index);
        *p = &after[delimiter.len_utf8()..];
        before
    } else {
        let before = *p;
        *p = "";
        before
    }
}

fn get_char(p: &mut &str) -> Option<char> {
    let ch = p.chars().next()?;
    *p = &p[ch.len_utf8()..];
    Some(ch)
}

fn skip_whitespace(p: &mut &str) {
    while let Some(ch) = p.chars().next() {
        if ch == ' ' {
            *p = &p[ch.len_utf8()..];
        } else {
            break;
        }
    }
}

pub fn parse(maps: &str) -> Vec<Region> {
    if maps.is_empty() {
        return Vec::new();
    }

    let mut output = Vec::new();
    for mut line in maps.trim().split('\n') {
        let start = u64::from_str_radix(get_until(&mut line, '-'), 16).unwrap();
        let end = u64::from_str_radix(get_until(&mut line, ' '), 16).unwrap();
        let is_read = get_char(&mut line).unwrap() == 'r';
        let is_write = get_char(&mut line).unwrap() == 'w';
        let is_executable = get_char(&mut line).unwrap() == 'x';
        let is_shared = get_char(&mut line).unwrap() == 's';
        get_char(&mut line);

        let file_offset = u64::from_str_radix(get_until(&mut line, ' '), 16).unwrap();
        let major = u32::from_str_radix(get_until(&mut line, ':'), 16).unwrap();
        let minor = u32::from_str_radix(get_until(&mut line, ' '), 16).unwrap();
        let inode = get_until(&mut line, ' ').parse().unwrap();
        skip_whitespace(&mut line);
        let name = line.to_owned();

        output.push(Region {
            start,
            end,
            is_read,
            is_write,
            is_executable,
            is_shared,
            file_offset,
            major,
            minor,
            inode,
            name,
        });
    }

    output
}

#[test]
fn test_get_until() {
    let mut p = "1234 5678";
    assert_eq!(get_until(&mut p, ' '), "1234");
    assert_eq!(p, "5678");

    assert_eq!(get_until(&mut p, ' '), "5678");
    assert_eq!(p, "");

    assert_eq!(get_until(&mut p, ' '), "");
}

#[test]
fn test_parse() {
    let maps = r#"
00400000-0040c000 r-xp 00000000 08:02 1321238                            /usr/bin/cat
0060d000-0062e000 rw-p 00000000 00:00 0                                  [heap]
7ffff672c000-7ffff69db000 r--s 00001ac2 1f:33 1335289                    /usr/lib/locale/locale-archive
7ffff5600000-7ffff5800000 rw-p 00000000 00:00 0
"#;

    assert_eq!(
        parse(maps),
        vec![
            Region {
                start: 0x00400000,
                end: 0x0040c000,
                is_read: true,
                is_write: false,
                is_executable: true,
                is_shared: false,
                file_offset: 0,
                major: 0x08,
                minor: 0x02,
                inode: 1321238,
                name: "/usr/bin/cat".to_owned()
            },
            Region {
                start: 0x0060d000,
                end: 0x0062e000,
                is_read: true,
                is_write: true,
                is_executable: false,
                is_shared: false,
                file_offset: 0,
                major: 0,
                minor: 0,
                inode: 0,
                name: "[heap]".to_owned()
            },
            Region {
                start: 0x7ffff672c000,
                end: 0x7ffff69db000,
                is_read: true,
                is_write: false,
                is_executable: false,
                is_shared: true,
                file_offset: 0x1ac2,
                major: 0x1f,
                minor: 0x33,
                inode: 1335289,
                name: "/usr/lib/locale/locale-archive".to_owned()
            },
            Region {
                start: 0x7ffff5600000,
                end: 0x7ffff5800000,
                is_read: true,
                is_write: true,
                is_executable: false,
                is_shared: false,
                file_offset: 0,
                major: 0,
                minor: 0,
                inode: 0,
                name: "".to_owned()
            }
        ]
    );
}

#[test]
fn test_empty_maps() {
    assert_eq!(parse(""), vec![]);
}
