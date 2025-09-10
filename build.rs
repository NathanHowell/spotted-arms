use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    // Emit git-related env vars (using the git CLI) and cargo info
    // We only need the short SHA, but emitting full git data is fine.
    use vergen::{CargoBuilder, Emitter};
    use vergen_gitcl::GitclBuilder;

    // Configure emission: all git info (includes VERGEN_GIT_SHA and VERGEN_GIT_SHA_SHORT)
    let git = GitclBuilder::all_git()?;

    Emitter::default()
        .add_instructions(&git)?
        // Also emit cargo info (CARGO_PKG_VERSION is provided by Cargo anyway)
        .add_instructions(&CargoBuilder::all_cargo()?)?
        .emit()?;

    Ok(())
}
