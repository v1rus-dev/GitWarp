## Install

Git is bundled; no separate install needed. Builds are not signed with a developer
certificate, so macOS and Windows show a warning on first launch.

| OS | Download |
|---|---|
| macOS (Apple Silicon, macOS 11+) | `gitwarp-<version>-macos-aarch64.dmg` |
| Windows (x86_64) | `gitwarp-<version>-windows-x86_64-setup.exe` (or the portable `.zip`) |
| Linux (x86_64, glibc 2.35+) | `gitwarp_<version>_amd64.deb`, `gitwarp-<version>-x86_64.AppImage` or `gitwarp-<version>-linux-x86_64.tar.gz` |

**macOS:** the easiest way is the install script. Files fetched with `curl` are not quarantined,
so GitWarp starts without any Gatekeeper prompt (`GITWARP_VERSION=X.Y.Z` pins a release):

```sh
curl -fsSL https://raw.githubusercontent.com/v1rus-dev/GitWarp/main/packaging/macos/install.sh | sh
```

Or open the downloaded `.dmg` and drag GitWarp to Applications. The app is not notarized, so macOS
refuses the first launch ("cannot verify that it is free of malware"); right-click → Open no
longer helps on macOS 15 and later. Either run in Terminal:

```sh
xattr -dr com.apple.quarantine /Applications/GitWarp.app
```

or try to open GitWarp once, click Done, then open System Settings → Privacy & Security, scroll
down and click **Open Anyway** next to the GitWarp message (it is shown for about an hour after
the blocked launch).

**Windows:** run the `-setup.exe`. If SmartScreen warns, click "More info", then "Run anyway".
The portable `.zip` needs no installation: unzip it and run `gitwarp.exe`.

**Linux:**

```sh
sudo apt install ./gitwarp_<version>_amd64.deb        # Debian / Ubuntu
chmod +x gitwarp-<version>-x86_64.AppImage && ./gitwarp-<version>-x86_64.AppImage
tar xzf gitwarp-<version>-linux-x86_64.tar.gz && cd gitwarp-<version>-linux-x86_64 && ./install.sh
```

Verify downloads against `SHA256SUMS.txt`:
`sha256sum -c SHA256SUMS.txt --ignore-missing`

---

