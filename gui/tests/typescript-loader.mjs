import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
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
  const overrides = JSON.parse(
    process.env.SSD_REFRESH_MUTATION_OVERRIDES ?? "{}",
  );
  const source = await readFile(
    overrides[fileURLToPath(url)] ?? new URL(url),
    "utf8",
  );
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
