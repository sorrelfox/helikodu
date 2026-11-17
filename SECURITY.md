# Security policy

## Supported versions

Helikodu is in early development and has no releases yet. Once it does, security fixes go into the latest release and, after 1.0, into the current stable branch.

## Reporting a vulnerability

**Please do not report security problems in public issues.**

Report them privately through GitHub's private vulnerability reporting: open the repository's **Security** tab and choose **Report a vulnerability**. Include:

- what the problem is and what an attacker could do with it;
- the steps or the file that reproduces it;
- the Helikodu version, your distribution, and the versions of the libraries involved (for example FFmpeg), if you know them.

You will get an answer within seven days. A confirmed vulnerability is fixed in a patch release and published as a security advisory, with credit to you if you wish.

Helikodu decodes media through system libraries such as FFmpeg. If the bug is in one of those libraries, it is also reported to that project.

## Scope

Helikodu has no network access, so anything that can harm a user arrives as a file (media files, tags, embedded pictures, playlists, CUE sheets, lyrics, presets) or through the D-Bus session bus. Examples of what counts as a vulnerability:

- memory corruption, a crash or a hang caused by a crafted file, in Helikodu's own code or in how it uses a decoder library;
- a playlist, CUE sheet or tag that makes Helikodu read, write, move or delete files outside what the user asked for;
- any way to make Helikodu open a network connection;
- output at full scale that the user did not ask for.
