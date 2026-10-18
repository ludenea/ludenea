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
    /// Writes `.ludenea/types/`: the game's types, next to the declarations of the modules
    /// plugins import. The game's types live in the type-only module `@ludenea/game`, so one
    /// named like an editor export (`Editor`) clashes with nothing; `@ludenea/editor-shell`
    /// only learns the stable name of each component and asset type through `ComponentTypes`
    /// and `AssetTypes`.
    pub fn write_game_types(&self, info: &GameInfo) -> Result<(), ProjectError> {
        let directory = self.root.join(".ludenea/types");
        fs::create_dir_all(&directory).map_err(|error| io_error(&directory, error))?;
        let components: Vec<_> = info
            .components
            .iter()
            .map(|component| (component.name.as_str(), &component.schema))
            .collect();
        let assets: Vec<_> = info
            .assets
            .iter()
            .map(|asset| (asset.name.as_str(), &asset.schema))
            .collect();
        let declarations = typescript_declarations(&components, &assets);
        let indented: String = declarations
            .lines()
            .map(|line| format!("  {line}\n"))
            .collect();
        let game = format!(
            "declare module \"@ludenea/game\" {{\n{indented}}}\n\n\
             declare module \"@ludenea/editor-shell\" {{\n  \
             import type {{ AssetTypes as GameAssetTypes, ComponentTypes as GameComponentTypes }} \
             from \"@ludenea/game\";\n  \
             interface ComponentTypes extends GameComponentTypes {{}}\n  \
             interface AssetTypes extends GameAssetTypes {{}}\n}}\n"
        );
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
