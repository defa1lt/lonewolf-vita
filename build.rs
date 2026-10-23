use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=native/vita_platform.c");
    println!("cargo:rerun-if-changed=native/boot_screen.h");
    println!("cargo:rerun-if-env-changed=VITASDK");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("vita") {
        return;
    }
    let sdk =
        PathBuf::from(env::var_os("VITASDK").expect("VITASDK must point to the VitaSDK directory"));
    println!("cargo:rustc-link-arg=-Wl,--defsym,__sce_headroom=0x10000");
    println!("cargo:rustc-link-arg=-Wl,--wrap=invoke_splashscreen");
    cc::Build::new()
        .compiler(sdk.join("bin/arm-vita-eabi-gcc"))
        .archiver(sdk.join("bin/arm-vita-eabi-ar"))
        .ranlib(sdk.join("bin/arm-vita-eabi-ranlib"))
        .file("native/vita_platform.c")
        .flag("-O2")
        .compile("lonewolf_platform");
    println!(
        "cargo:rustc-link-search=native={}",
        sdk.join("arm-vita-eabi/lib").display()
    );
    for library in [
        "vitaGL",
        "vitashark",
        "mathneon",
        "SceShaccCgExt",
        "taihen_stub",
        "SceGxm_stub",
        "SceDisplay_stub",
        "SceSysmem_stub",
        "SceCommonDialog_stub",
        "SceAppMgr_stub",
        "SceCtrl_stub",
        "SceTouch_stub",
        "SceMotion_stub",
        "SceAudio_stub",
        "ScePower_stub",
        "SceRtc_stub",
        "SceKernelThreadMgr_stub",
        "SceLibKernel_stub",
        "SceKernelDmacMgr_stub",
        "SceIofilemgr_stub",
        "SceShaccCg_stub",
        "ScePgf_stub",
    ] {
        println!("cargo:rustc-link-lib=static={library}");
    }
    println!("cargo:rustc-link-lib=stdc++");
    println!("cargo:rustc-link-lib=m");
}
