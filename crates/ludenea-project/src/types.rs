use std::fs;

use ludenea_control::GameInfo;
use ludenea_schema::typescript_declarations;

use crate::{Project, ProjectError, io_error};

const SHARED_DECLARATIONS: [(&str, &str); 3] = [
    (
        "editor-shell.d.ts",
        include_str!("../types/editor-shell.d.ts"),
    ),
    (
        "control-client.d.ts",
        include_str!("../types/control-client.d.ts"),
    ),
    ("react.d.ts", include_str!("../types/react.d.ts")),
];

impl Project {
    /// Writes `.ludenea/types/`: the game's component types, which plugins see through
    /// `@ludenea/editor-shell`, next to the declarations of the modules plugins import.
    pub fn write_game_types(&self, info: &GameInfo) -> Result<(), ProjectError> {
        let directory = self.root.join(".ludenea/types");
        fs::create_dir_all(&directory).map_err(|error| io_error(&directory, error))?;
        let components: Vec<_> = info
            .components
            .iter()
            .map(|component| (component.name.as_str(), &component.schema))
            .collect();
        let declarations = typescript_declarations(&components);
        let indented: String = declarations
            .lines()
            .map(|line| format!("  {line}\n"))
            .collect();
        let game = format!("declare module \"@ludenea/editor-shell\" {{\n{indented}}}\n");
        let files = SHARED_DECLARATIONS
            .into_iter()
            .chain([("game.d.ts", game.as_str())]);
        for (name, text) in files {
            let path = directory.join(name);
            fs::write(&path, text).map_err(|error| io_error(&path, error))?;
        }
        Ok(())
    }
}
