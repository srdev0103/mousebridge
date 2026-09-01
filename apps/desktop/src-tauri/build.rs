//! Build script: generates the Tauri context, embeds the frontend and, on
//! Windows, links the application manifest and resources.

fn main() {
    tauri_build::build();
}
