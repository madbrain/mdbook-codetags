use std::{io, process};

use clap::{Arg, ArgMatches, Command};
use mdbook::{errors::Error, preprocess::{CmdPreprocessor, Preprocessor}};
use semver::{Version, VersionReq};

mod preprocessor;
mod config;

fn cmd() -> Command {
    Command::new("codetags")
        .about(clap::crate_description!())
        .author(clap::crate_authors!())
        .version(clap::crate_version!())
        .subcommand(
            Command::new("supports")
                .arg(Arg::new("renderer").required(true))
                .about("Check whether a renderer is supported by this preprocessor"),
        )
}

fn main() {
    env_logger::init();

    let matches = cmd().get_matches();
    let preproc = preprocessor::CodeTagsHighlighterPreprocessor;

    if let Some(sub_args) = matches.subcommand_matches("supports") {
        handle_supports(&preproc, sub_args);
    } else if let Err(e) = handle_preprocessing(&preproc) {
        log::error!("{}", e);
        process::exit(1);
    }
}

fn handle_preprocessing(pre: &dyn Preprocessor) -> Result<(), Error> {

    // <debug>
    // let mut file = std::fs::File::create("dump.json").unwrap();
    // for line in io::stdin().lines() {
    //     file.write_all(line?.as_bytes())?;
    // }
    // file.flush().unwrap();
    // </debug>

    let (ctx, book) = CmdPreprocessor::parse_input(io::stdin())?;

    let book_version = Version::parse(&ctx.mdbook_version)?;
    let version_req = VersionReq::parse(mdbook::MDBOOK_VERSION)?;

    if !version_req.matches(&book_version) {
        log::warn!(
            "Warning: The {} plugin was built against version {} of mdbook, \
             but we're being called from version {}",
            pre.name(),
            mdbook::MDBOOK_VERSION,
            ctx.mdbook_version
        );
    }

    let processed_book = pre.run(&ctx, book)?;
    serde_json::to_writer(io::stdout(), &processed_book)?;

    Ok(())
}

fn handle_supports(pre: &dyn Preprocessor, sub_args: &ArgMatches) -> ! {
    let renderer = sub_args
        .get_one::<String>("renderer")
        .expect("Required argument");
    let supported = pre.supports_renderer(renderer);

    // Signal whether the renderer is supported by exiting with 1 or 0.
    if supported {
        process::exit(0);
    } else {
        process::exit(1);
    }
}