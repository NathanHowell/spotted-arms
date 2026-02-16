use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    // Emit git-related env vars (using the git CLI) and cargo info
    // We only need the short SHA, but emitting full git data is fine.
    use vergen::{Cargo, Emitter};
    use vergen_gitcl::Gitcl;

    let git = Gitcl::all_git();

    Emitter::default()
        .add_instructions(&git)?
        .add_instructions(&Cargo::all_cargo())?
        .emit()?;

    Ok(())
}
