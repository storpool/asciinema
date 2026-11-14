use std::io::{self, Write};
use std::path::Path;

use anyhow::Result;

use crate::{asciicast, cli, history, util};

impl cli::History {
    pub fn run(self) -> Result<()> {
        let path: Box<dyn AsRef<Path>> = if self.input == "-" {
            Box::new(Path::new("/dev/stdin"))
        } else {
            util::get_local_path(&self.input)?
        };

        let commands = history::extract(asciicast::open_from_path(path.as_ref().as_ref())?)?;
        let mut stdout = io::stdout().lock();

        if self.json {
            serde_json::to_writer_pretty(&mut stdout, &commands)?;
            writeln!(stdout)?;
        } else {
            for command in commands {
                writeln!(stdout, "{}", command.replace('\n', "\\n"))?;
            }
        }

        Ok(())
    }
}
