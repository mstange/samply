use super::elevated_helper::ElevatedRecordingProps;

pub fn gfx_xperf_args(props: &ElevatedRecordingProps) -> Vec<String> {
    let mut providers = vec![];

    if !props.gfx {
        return providers;
    }

    const DXGKRNL_BASE_KEYWORD: u64 = 0x1;

    // er I don't know what level 1 is.
    let level_1_dxgkrnl_keywords = DXGKRNL_BASE_KEYWORD;

    if level_1_dxgkrnl_keywords != 0 {
        providers.push(format!(
            "Microsoft-Windows-DxgKrnl:0x{:x}:1",
            level_1_dxgkrnl_keywords
        ));
    }

    providers
}
