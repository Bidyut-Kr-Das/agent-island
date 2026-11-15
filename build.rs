fn main() {
    // The hook path never draws; loading opengl32.dll (and the GPU driver it pulls in) at startup cost
    // about 65 ms per hook call. Load it on first use instead.
    println!("cargo:rustc-link-arg-bins=/DELAYLOAD:opengl32.dll");
    println!("cargo:rustc-link-arg-bins=delayimp.lib");
}
