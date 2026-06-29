// Postinstall: fetch the prebuilt tripwire-server binary for this platform from the
// matching GitHub Release and unpack it into bin/. The package version is the release
// tag, so `npm i tripwire-roblox@x.y.z` always pulls the binary built for x.y.z.

const fs = require("fs");
const path = require("path");
const https = require("https");
const { execFileSync } = require("child_process");

const pkg = require("./package.json");
const REPO = "aliboIly/Tripwire";
const tag = "v" + pkg.version;

// node platform-arch -> Rust target triple built by .github/workflows/release.yml
const TARGETS = {
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
  "linux-x64": "x86_64-unknown-linux-gnu",
  "win32-x64": "x86_64-pc-windows-msvc",
};

function fail(message) {
  console.error(`[tripwire-roblox] ${message}`);
  console.error("[tripwire-roblox] Build from source instead: https://github.com/aliboIly/Tripwire");
  process.exit(1);
}

const platformKey = `${process.platform}-${process.arch}`;
const target = TARGETS[platformKey];
if (!target) {
  fail(`no prebuilt binary for ${platformKey}.`);
}

const binDir = path.join(__dirname, "bin");
const binName = process.platform === "win32" ? "tripwire-server.exe" : "tripwire-server";
const binPath = path.join(binDir, binName);
if (fs.existsSync(binPath)) {
  // Already installed (cached npx run, or reinstall).
  process.exit(0);
}

fs.mkdirSync(binDir, { recursive: true });
const asset = `tripwire-server-${tag}-${target}.tar.gz`;
const url = `https://github.com/${REPO}/releases/download/${tag}/${asset}`;
const archivePath = path.join(binDir, asset);

function download(from, dest, done, redirects = 0) {
  if (redirects > 5) {
    return fail("too many redirects while downloading the binary.");
  }
  https
    .get(from, { headers: { "User-Agent": "tripwire-roblox-installer" } }, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        res.resume();
        return download(res.headers.location, dest, done, redirects + 1);
      }
      if (res.statusCode !== 200) {
        res.resume();
        return fail(`download failed (HTTP ${res.statusCode}) for ${from}`);
      }
      const file = fs.createWriteStream(dest);
      res.pipe(file);
      file.on("finish", () => file.close(done));
      file.on("error", (e) => fail(e.message));
    })
    .on("error", (e) => fail(e.message));
}

download(url, archivePath, () => {
  try {
    // tar is present on macOS, Linux, and Windows 10+ (bsdtar).
    execFileSync("tar", ["-xzf", archivePath, "-C", binDir], { stdio: "inherit" });
    fs.unlinkSync(archivePath);
    if (!fs.existsSync(binPath)) {
      return fail("the binary was missing from the downloaded archive.");
    }
    if (process.platform !== "win32") {
      fs.chmodSync(binPath, 0o755);
    }
    console.log(`[tripwire-roblox] installed ${target} (${tag}).`);
  } catch (e) {
    fail(`could not unpack the binary: ${e.message}`);
  }
});
