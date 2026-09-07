use std::path::PathBuf;

pub fn home_dir() -> Option<PathBuf> {
    crate::android_home_dir()
}

pub fn cache_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join("cache"))
}

pub fn config_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join("config"))
}

pub fn data_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join("data"))
}

pub fn data_local_dir() -> Option<PathBuf> {
    data_dir()
}

pub fn runtime_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join("runtime"))
}

pub fn executable_dir() -> Option<PathBuf> {
    None
}

pub fn audio_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join("Audio"))
}

pub fn desktop_dir() -> Option<PathBuf> {
    None
}

pub fn document_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join("Documents"))
}

pub fn download_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join("Downloads"))
}

pub fn font_dir() -> Option<PathBuf> {
    data_dir().map(|data| data.join("fonts"))
}

pub fn picture_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join("Pictures"))
}

pub fn public_dir() -> Option<PathBuf> {
    None
}

pub fn template_dir() -> Option<PathBuf> {
    None
}

pub fn video_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join("Videos"))
}
