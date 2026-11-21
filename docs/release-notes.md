<!-- Notes for the next release. The Release workflow publishes this file as the
     GitHub release body and as the `notes` field of latest.json, which is what
     Kept's update window shows, so rewrite it before tagging. -->

## The new mark

The app icon, the tray, the title bar logo and the settings header all carry the Kept mark now, redrawn from the vector original so every bundle size stays sharp.

## Saving follows the clip

- Links save as Windows shortcuts, colours as text or CSS, images as capture-<date>.png
- A GIF or video clip saves the moving asset, with the still frame as the second choice
- Secrets ask before they are written outside the vault, and sealed images are decrypted rather than copied as ciphertext
- A file clip reports the originals that had gone instead of failing the whole export
- Batch saves can sort into Images, Text and Files, and the last folder is remembered

## A window for updates

The launch check opens the release notes in a window of their own, with Update now, Skip this version and Remind me later. Settings > About also has "Show latest updates", which embeds the same page.

## Cloud folders warn first

Choosing a data folder inside Dropbox or Google Drive explains what a sync client does to a live database, and offers Cancel.
