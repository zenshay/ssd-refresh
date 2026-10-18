import { readFile } from "node:fs/promises";
import ts from "typescript";
export async function resolve(specifier, context, next) {
  if (
    (specifier.startsWith("./") || specifier.startsWith("../")) &&
    !/\.[a-z]+$/i.test(specifier)
  )
    return next(`${specifier}.ts`, context);
  return next(specifier, context);
}
export async function load(url, context, next) {
  if (!url.endsWith(".ts")) return next(url, context);
  const source = await readFile(new URL(url), "utf8");
  return {
    format: "module",
    shortCircuit: true,
    source: ts.transpileModule(source, {
      compilerOptions: {
        module: ts.ModuleKind.ESNext,
        target: ts.ScriptTarget.ES2021,
      },
    }).outputText,
  };
}
