import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const vendored = ["skua/vendor/Skua", "skua/vendor/Scripts"];

function git(...args: string[]): string {
  return execFileSync("git", args, { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] });
}

git("submodule", "sync", "--quiet", "--", ...vendored);
git("submodule", "update", "--init", "--remote", "--depth", "1", "--", ...vendored);
for (const dir of vendored) {
  console.log(`${dir}: ${git("-C", dir, "log", "-1", "--format=%h %cs %s").trim()}`);
}
