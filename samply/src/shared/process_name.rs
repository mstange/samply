#[allow(dead_code)]
pub fn make_process_name(
    executable: &str,
    args: Vec<String>,
    arg_count_to_include: usize,
) -> String {
    let mut args = args.iter().map(std::ops::Deref::deref);
    let _executable = args.next();
    let mut included_args = args.take(arg_count_to_include).peekable();
    if included_args.peek().is_some() {
        let joined_args = shlex::try_join(included_args).unwrap_or_default();
        format!("{executable} {joined_args}")
    } else {
        executable.to_owned()
    }
}
