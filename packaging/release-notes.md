## Install

Git is bundled; no separate install needed. Builds are not signed with a developer
certificate, so Windows shows a warning on first launch.

| OS | Download |
|---|---|
| macOS (Apple Silicon, macOS 11+) | install with Homebrew (below), do not download the `.dmg` |
| Windows (x86_64) | `gitwarp-<version>-windows-x86_64-setup.exe` (or the portable `.zip`) |
| Linux (x86_64, glibc 2.35+) | `gitwarp_<version>_amd64.deb`, `gitwarp-<version>-x86_64.AppImage` or `gitwarp-<version>-linux-x86_64.tar.gz` |

**macOS:** install with Homebrew:

```sh
brew install --cask v1rus-dev/tap/gitwarp
open -a GitWarp
```

Do not download the macOS `.dmg` or `.zip` attached below: the app is not notarized, so macOS
refuses to open a build downloaded in a browser. Homebrew and the in-app updater use those files.

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

