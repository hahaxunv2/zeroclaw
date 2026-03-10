use std::path::Path;
use zeroclaw::security::secrets::SecretStore;

fn main() -> anyhow::Result<()> {
    let zeroclaw_dir = Path::new("/Users/yunyun/.zeroclaw");
    let store = SecretStore::new(zeroclaw_dir, true);

    // Encrypted key from config.toml
    let encrypted = "enc2:7dd0c2147be368e430bdf3af87132345a1fd5e0ac442b33c03e6d94f193733867430d69cf8e1c471dfdb7a4ccc420f4c728caa126b019541dc1fefe45907c6870fc98c";

    let decrypted = store.decrypt(encrypted)?;
    println!("RAW_KEY: {}", decrypted);

    Ok(())
}
