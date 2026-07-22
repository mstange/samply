use std::env;
use std::io::Write;

const ENTITLEMENTS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>com.apple.security.cs.debugger</key>
	<true/>
</dict>
</plist>
"#;

pub fn codesign_setup(skip_prompt: bool) {
    let exe_path = env::current_exe().unwrap();

    if !skip_prompt {
        print!(
            r#"
On macOS, attaching to an existing process is only allowed to binaries with
the com.apple.security.cs.debugger entitlement. The samply binary will be
signed with this entitlement for your local machine only. The following command
will be run:

    codesign --force --options runtime --sign - \
      --entitlements entitlements.xml {}

entitlements.xml contains:

    {}

Press any key to continue, or Ctrl-C to cancel.
"#,
            exe_path.display(),
            ENTITLEMENTS_XML
        );

        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .expect("Failed to read input?");
    }

    let mut entitlements_file = tempfile::Builder::new()
        .prefix("samply_entitlements")
        .suffix(".xml")
        .tempfile()
        .expect("Failed to create temporary file for entitlements!");

    entitlements_file
        .write_all(ENTITLEMENTS_XML.as_bytes())
        .expect("Failed to write entitlements to temporary file!");

    let entitlements_path = entitlements_file.path();

    // codesign for the current machine:
    //    codesign --force --options runtime --sign - --entitlements ent.xml target/debug/usamply
    let output = std::process::Command::new("codesign")
        .arg("--force")
        .arg("--options")
        .arg("runtime")
        .arg("--sign")
        .arg("-")
        .arg("--entitlements")
        .arg(entitlements_path)
        .arg(&exe_path)
        .output()
        .expect("Failed to run codesign!");

    if !output.status.success() {
        println!(
            "codesign failed with status: {}\noutput:\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    println!(r"Code signing successful!");
}
