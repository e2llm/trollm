fn main() {
    let directory = "vendor/rpa-kernel-clock";
    for file in [
        "kernel-clock-probe.c",
        "kernel-clock-reader.h",
        "source.json",
    ] {
        println!("cargo:rerun-if-changed={directory}/{file}");
    }
    println!("cargo:rerun-if-changed=src/kernel-clock-layout.c");
    if matches!(
        std::env::var("CARGO_CFG_TARGET_OS").as_deref(),
        Ok("linux" | "android")
    ) {
        // cargo-ndk supplies the target compiler through the existing Android
        // build. Production never defines an adjtimex test override.
        cc::Build::new()
            .file(format!("{directory}/kernel-clock-probe.c"))
            .file("src/kernel-clock-layout.c")
            .include(directory)
            .std("c11")
            .define("MORAY_KERNEL_CLOCK_NO_MAIN", None)
            .warnings(true)
            .warnings_into_errors(true)
            .compile("moray_kernel_clock_reader_v1");
    }
}
