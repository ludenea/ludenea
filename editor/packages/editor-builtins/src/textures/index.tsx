import { definePlugin } from "@ludenea/editor-shell";
import { SpriteSheetClips } from "./ClipPlayer";
import { TexturePreview } from "./TexturePreview";

/** Texture previews and animation clip players in the inspector, on the asset inspector API any plugin gets. */
export const texturesPlugin = definePlugin({
  name: "ludenea.textures",
  activate(editor) {
    editor.inspectors.registerAsset("Texture", ({ id }) => <TexturePreview editor={editor} id={id} />);
    editor.inspectors.registerAsset("ludenea::SpriteSheet", ({ value }) => <SpriteSheetClips editor={editor} value={value} />);
  },
});
