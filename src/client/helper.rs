use scrap::CodecFormat;
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct QualityStatus {
    pub speed: Option<String>,
    pub fps: HashMap<usize, i32>,
    pub delay: Option<i32>,
    pub target_bitrate: Option<i32>,
    pub codec_format: Option<CodecFormat>,
    pub chroma: Option<String>,
}
