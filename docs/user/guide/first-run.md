# First run and music folders

When Helikodu starts for the first time, your library is empty, and the window asks **Where is your music?**

1. Your music folder (such as `~/Music`) is listed and ticked. **Add another folder** adds more; untick a folder to leave it out.
2. **Watch these folders** keeps the library up to date on its own, and **Include video files** adds the sound tracks of video files.
3. The window also says which device the sound goes to; **Choose device** picks another.
4. **Scan and start** reads the folders and opens the library. **Skip for now** opens the empty library.

Helikodu reads the folders and their subfolders. Your files are played where they are; nothing is copied or moved. Albums appear as they are found, while **Scanning library** at the bottom of the sidebar shows the progress. A large collection can take a few minutes the first time. Later starts only read files that are new or changed.

To add folders later, choose **Add Folder to Library…** in the main menu, or open **Settings**, go to the **Library** page and choose **Add folder** under **Folders**. You can add as many folders as you like, and each one can be scanned again or removed from its row. Removing a folder only removes it from the library, never from your disk.

## Keeping the library up to date

With **Watch the folders for changes** on (the default), new, changed, moved and deleted files show up on their own within about a second. Folders on network shares and FUSE file systems cannot be watched; they are scanned again at a regular interval instead (every 30 minutes by default). **Rescan all** in **Settings → Library**, or **Library ▸ Rescan** in the main menu, scans every folder again on request.

## What is scanned

- Audio files, and CUE sheets, which split a single album file into its tracks.
- Video files only when **Include video files** is on. Their sound track is played; no picture is ever shown.
- Hidden files and folders (names that start with a dot) and folders that contain a file named `.nomedia` are skipped.
- **Skip** takes name patterns to leave out, separated by semicolons, with `*` and `?` as wildcards, for example `*.part; Podcasts/`.
- Symbolic links that point outside your library folders are skipped unless **Follow symbolic links** is on.

## Drives that come and go

If a library folder is on a USB drive or a disk that is not mounted, Helikodu notices that it is gone and, with **Hide tracks on drives that are not connected** on, hides its tracks instead of deleting them. When the drive is back, they return with their ratings and play counts.

A single file that disappears is kept in the library for 30 days in case it comes back (for example after you move it out and back again). Moved and renamed files keep their play counts, ratings and playlist places.
