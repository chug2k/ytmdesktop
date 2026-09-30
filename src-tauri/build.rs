fn main() {
    #[cfg(target_os = "macos")]
    {
        // tauri_build declares its own inputs, which turns off Cargo's default
        // of rerunning on any change. Without this line, edits to the .m file
        // are silently not compiled.
        println!("cargo:rerun-if-changed=src/macos_notifications.m");
        cc::Build::new()
            .file("src/macos_notifications.m")
            .flag("-fobjc-arc")
            .flag("-mmacosx-version-min=11.0")
            .compile("macos_notifications");

        println!("cargo:rerun-if-changed=src/macos_bluetooth.m");
        cc::Build::new()
            .file("src/macos_bluetooth.m")
            .flag("-fobjc-arc")
            .flag("-mmacosx-version-min=11.0")
            .compile("macos_bluetooth");

        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=CoreBluetooth");
        println!("cargo:rustc-link-lib=framework=UserNotifications");
    }

    tauri_build::build();
}
