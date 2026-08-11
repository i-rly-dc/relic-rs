use std::path::Path;

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    for f in ["text-detection.rten", "text-recognition.rten"] {
        let p = Path::new(&manifest).join("models").join(f);
        if !p.exists() {
            panic!(
                "\n\nmissing OCR model: {}\nrun ./fetch-models.sh once before building.\n",
                p.display()
            );
        }
    }
    println!("cargo:rerun-if-changed=models");
}
