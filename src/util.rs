use std::path::PathBuf;

pub fn get_socket_path() -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push("aerotag.sock");
    path
}
