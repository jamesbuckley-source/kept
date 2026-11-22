# Demo videos

Short clips of Kept in use, recorded for the README and release notes.

| File | Shows |
| --- | --- |
| `kept-paste-stack.mp4` | Copying a handful of clips, then pasting them back in order with Paste stack. |
| `kept-colour-palette.mp4` | Pulling the colour scheme out of a screenshot and keeping it as an archive of colour clips. |
| `kept-copy-from-web.mp4` | Copying samples from [`demo.html`](../demo.html) with Kept open beside the page, and watching them land in the history. |

## Recording rules

- **Never record on a real history.** Personal clips must not appear in a demo, a
  screenshot or a bug report. Switch to the demonstration database first.
- The demonstration database lives at `%LOCALAPPDATA%\kept-demo\data` and is
  deliberately _not_ deleted between sessions, so every run starts from the same
  history.

## The demonstration database

[`scripts/demo-db.ps1`](../../scripts/demo-db.ps1) switches Kept between the real
history and the demo one. It rewrites the data-folder pointer in `storage.json`
rather than copying anything, so the real database is never touched.

```powershell
.\scripts\demo-db.ps1 -Use      # record on the demo database
.\scripts\demo-db.ps1 -Reset    # rebuild it clean (new database + seeded filler)
.\scripts\demo-db.ps1 -Status   # which database would Kept open?
.\scripts\demo-db.ps1 -Restore  # back to your own history
```

`-Reset` before any demo that creates archives, stacks or collections, so the next
recording starts from the same state as the last one. Restart Kept after switching,
or pass `-NoRestart` and start it yourself - the pointer is only read at startup.
