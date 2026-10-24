import assert from "node:assert/strict";
import test from "node:test";
import { convert, findProblems } from "./check-eol.mjs";

const record = (index, work, attr, path) =>
  `${index} ${work} attr/${attr}\t${path}\0`;

test("LF text files, binary files and CRLF .bat files pass", () => {
  const out =
    record("i/lf", "w/lf", "text=auto eol=lf", "src/main.rs") +
    record("i/-text", "w/-text", "-text", "gui/icon.png") +
    record("i/none", "w/none", "text=auto eol=lf", "empty.txt") +
    record("i/lf", "w/crlf", "text eol=crlf", "run.bat");
  assert.deepEqual(findProblems(out), []);
});

test("CRLF in the work tree or the index fails", () => {
  const out =
    record("i/lf", "w/crlf", "text=auto eol=lf", "src/a.rs") +
    record("i/crlf", "w/crlf", "text=auto eol=lf", "b.md") +
    record("i/lf", "w/mixed", "text=auto eol=lf", "c with space.md");
  const problems = findProblems(out).map((p) => p.text);
  assert.deepEqual(problems, [
    "src/a.rs: the work tree has CRLF",
    "b.md: the index has CRLF",
    "b.md: the work tree has CRLF",
    "c with space.md: the work tree has MIXED",
  ]);
});

test("A .bat file with LF in the work tree fails", () => {
  const out = record("i/lf", "w/lf", "text eol=crlf", "run.bat");
  assert.deepEqual(findProblems(out), [
    {
      path: "run.bat",
      crlf: true,
      text: "run.bat: the work tree has LF, but the file must use CRLF",
    },
  ]);
});

test("convert gives LF or CRLF line endings", () => {
  assert.equal(convert("a\r\nb\nc\r\n", false), "a\nb\nc\n");
  assert.equal(convert("a\r\nb\nc", true), "a\r\nb\r\nc");
});
