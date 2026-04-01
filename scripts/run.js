#!/usr/bin/env node
const { execFileSync } = require("child_process");
const fs = require("fs");
const path = require("path");

const ext = process.platform === "win32" ? ".exe" : "";
const bin = path.join(__dirname, "..", "bin", "browsectl" + ext);

if (!fs.existsSync(bin)) {
  console.error(
    "browsectl binary not found. The postinstall script may not have run.\n" +
      "\n" +
      "If you installed with pnpm, approve build scripts first:\n" +
      "  pnpm approve-builds -g\n" +
      "\n" +
      "Or re-install with scripts allowed:\n" +
      "  pnpm install -g @yorelog/browsectl --ignore-scripts=false\n" +
      "\n" +
      "You can also run the install script manually:\n" +
      "  node " +
      path.join(__dirname, "install.js"),
  );
  process.exit(1);
}

try {
  execFileSync(bin, process.argv.slice(2), { stdio: "inherit" });
} catch (e) {
  process.exit(e.status || 1);
}
