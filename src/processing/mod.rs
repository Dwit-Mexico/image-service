pub mod audio;
pub mod image;
pub mod video;

pub use audio::{process_audio, AudioOptions, AudioResult};
pub use image::{process_image, ProcessOptions};
pub use video::{process_video, VideoOptions, VideoResult};
