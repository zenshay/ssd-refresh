// A build error or a timeout does not prove that a test found a mutation.
export function mutationStatus(result, language) {
  if (result.timedOut) return "timeout";
  if (result.code === 0) return "survived";
  const assertionFailed =
    language === "rust"
      ? /test result: FAILED/.test(result.text)
      : /not ok \d+/.test(result.text) && /ERR_ASSERTION/.test(result.text);
  return assertionFailed ? "killed" : "invalid";
}
