use std::env;

fn main() -> Result<(), std::io::Error> {
    for (key, value) in env::vars() {
        println!("{}: {}", key, value);
    }
    
    tonic_flatbuffers_build::configure()
        .emit_rerun_if_changed(true)
        .generate_default_stubs(true)
        .out_dir("src/generated")
        .compile(&["./fbs/shared.fbs", "./fbs/service.fbs"])?;

    Ok(())
}
