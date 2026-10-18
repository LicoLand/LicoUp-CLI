import process from "node:process";

export function parseReleaseProofArgs(argv = process.argv.slice(2)) {
  const args = new Set(argv);
  return {
    strict: args.has("--strict"),
    clientLicoArcCryptoReadinessSelfTest: args.has(
      "--client-licoarc-crypto-readiness-self-test",
    ),
    releaseProofContractReadinessSelfTest: args.has(
      "--release-proof-contract-readiness-self-test",
    ),
  };
}
