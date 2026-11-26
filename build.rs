use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=vita-stubs/SceVideodecLowDelayUser.S");
    let target = env::var("TARGET").unwrap_or_default();
    if !target.contains("vita") {
        return;
    }


    // sceAvcdecSetLowDelayModeNongameapp lives in SceVideodecLowDelayUser (NID 0x6354EEFF),
    // not the SceVideodecUser library the SDK stub imports from. Assemble a correct import
    // stub (as/ar only; the SDK's C compiler is broken on this host).
    let vitasdk = PathBuf::from(env::var("VITASDK").expect("VITASDK env var is not set"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let stub_src = manifest_dir.join("vita-stubs/SceVideodecLowDelayUser.S");
    let obj = out_dir.join("SceVideodecLowDelayUser.o");
    let lib = out_dir.join("libSceVideodecLowDelay_stub.a");

    let status = Command::new(vitasdk.join("bin/arm-vita-eabi-as"))
        .arg("--defsym")
        .arg("GEN_WEAK_EXPORTS=0")
        .arg(&stub_src)
        .arg("-o")
        .arg(&obj)
        .status()
        .expect("failed to run arm-vita-eabi-as");
    assert!(status.success(), "arm-vita-eabi-as failed for low-delay stub");

    let status = Command::new(vitasdk.join("bin/arm-vita-eabi-ar"))
        .arg("crus")
        .arg(&lib)
        .arg(&obj)
        .status()
        .expect("failed to run arm-vita-eabi-ar");
    assert!(status.success(), "arm-vita-eabi-ar failed for low-delay stub");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=SceVideodecLowDelay_stub");
}
