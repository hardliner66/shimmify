use shimmify::shimmify;

fn main() -> anyhow::Result<()> {
    shimmify("shimmify.toml");

    println!("Unshimmed!");
    Ok(())
}
