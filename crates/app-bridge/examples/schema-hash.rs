use std::path::PathBuf;

use webtest_app_bridge::AppManifest;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let first = arguments
        .next()
        .ok_or("usage: schema-hash [--write] <app-schema.json>")?;
    let (write, path) = if first == "--write" {
        (
            true,
            arguments
                .next()
                .map(PathBuf::from)
                .ok_or("usage: schema-hash --write <app-schema.json>")?,
        )
    } else {
        (false, PathBuf::from(first))
    };
    if arguments.next().is_some() {
        return Err("usage: schema-hash [--write] <app-schema.json>".into());
    }
    let source = std::fs::read_to_string(&path)?;
    let manifest: AppManifest = serde_json::from_str(&source)?;
    let manifest = manifest.with_computed_hash()?;
    manifest.validate()?;
    if write {
        let mut output = serde_json::to_string_pretty(&manifest)?;
        output.push('\n');
        std::fs::write(&path, output)?;
    }
    println!("{}", manifest.schema_hash);
    Ok(())
}
