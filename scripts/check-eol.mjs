// Check the line endings of all files in Git.
// Text files must use LF in the index and in the work tree.
// Files with the attribute eol=crlf (.bat, .cmd) must use CRLF in the work tree.
// With --fix, the script corrects the files. Then it adds them to the index again.
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { capture, root } from "./tools.mjs";

/** Give the text with LF line endings, or with CRLF line endings if crlf is true. */
export function convert(text, crlf) {
  const lf = text.replace(/\r\n/g, "\n");
  return crlf ? lf.replace(/\n/g, "\r\n") : lf;
}

/**
 * Find the problems in the output of `git ls-files --eol -z`.
 * Returns one item per problem: { path, crlf, text }. crlf = the file must use CRLF.
 */
export function findProblems(output) {
  const problems = [];
  for (const record of output.split("\0")) {
    if (!record) continue;
    const tab = record.indexOf("\t");
    if (tab < 0) throw new Error(`Unknown git ls-files record: ${record}`);
    const [index, work, ...attr] = record.slice(0, tab).trim().split(/\s+/);
    const path = record.slice(tab + 1);
    const crlf = attr.join(" ").includes("eol=crlf");
    const name = (value) => value.slice(2).toUpperCase();
    if (index === "i/crlf" || index === "i/mixed") {
      problems.push({
        path,
        crlf,
        text: `${path}: the index has ${name(index)}`,
      });
    }
    if (!crlf && (work === "w/crlf" || work === "w/mixed")) {
      problems.push({
        path,
        crlf,
        text: `${path}: the work tree has ${name(work)}`,
      });
    }
    if (crlf && (work === "w/lf" || work === "w/mixed")) {
      problems.push({
        path,
        crlf,
        text: `${path}: the work tree has ${name(work)}, but the file must use CRLF`,
      });
    }
  }
  return problems;
}

function git(args) {
  const result = capture("git", args);
  if (result.status !== 0) {
    throw new Error(`git ${args[0]} failed: ${result.stderr}`);
  }
  return result.stdout;
}

function main() {
  const list = () => findProblems(git(["ls-files", "--eol", "-z"]));
  let problems = list();
  if (problems.length > 0 && process.argv.includes("--fix")) {
    const paths = [...new Set(problems.map((p) => p.path))];
    for (const problem of problems) {
      const file = join(root, problem.path);
      // latin1 keeps each byte. Only the CR and LF bytes change.
      const text = readFileSync(file, "latin1");
      writeFileSync(file, convert(text, problem.crlf), "latin1");
    }
    git(["add", "--renormalize", "--", ...paths]);
    for (const path of paths) console.log(`FIXED: ${path}`);
    problems = list();
  }
  if (problems.length > 0) {
    for (const problem of problems) console.log(`EOL: ${problem.text}`);
    console.log("To correct the files: node scripts/check-eol.mjs --fix");
    console.log(`RESULT: NO (${problems.length} problems)`);
    process.exit(1);
  }
  console.log("RESULT: YES (line endings)");
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) main();
