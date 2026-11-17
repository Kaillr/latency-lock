// semantic-release "prepare" step: stamp the version into Cargo.toml and Cargo.lock, build the
// release exe, and write the .sha256 that the in-app updater checks downloads against.
// Usage: node .github/scripts/prepare-release.mjs 1.2.3

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";

const version = process.argv[2];
if (!/^\d+\.\d+\.\d+$/.test(version ?? "")) {
  throw new Error(`expected a version like 1.2.3, got ${version}`);
}

function stamp(file, pattern) {
  const text = readFileSync(file, "utf8");
  if (!pattern.test(text)) throw new Error(`no package version found in ${file}`);
  writeFileSync(file, text.replace(pattern, `$1"${version}"`));
}

// The first `version =` in Cargo.toml is the [package] one.
stamp("Cargo.toml", /^(version = )"[^"]*"/m);
stamp("Cargo.lock", /(\[\[package\]\]\r?\nname = "latency-lock"\r?\nversion = )"[^"]*"/);

execFileSync("cargo", ["build", "--release", "--locked"], { stdio: "inherit" });

const exe = "target/release/latency-lock.exe";
const hash = createHash("sha256").update(readFileSync(exe)).digest("hex");
writeFileSync(`${exe}.sha256`, `${hash}  latency-lock.exe\n`);
console.log(`latency-lock.exe ${version} sha256 ${hash}`);
