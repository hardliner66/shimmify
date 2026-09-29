use shimmify::shimmify;

fn main() -> anyhow::Result<()> {
    shimmify("shimmify.toml", None);

    println!("Unshimmed!");
    Ok(())
}
