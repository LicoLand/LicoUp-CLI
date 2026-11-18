import { definePlatformEntry } from "../factory.mjs";
export default definePlatformEntry({
  id: "macos",
  hosts: ["darwin"],
  unverifiedInputs: [
    "tests/product-e2e/cli/client-update/nightly-self-migration-installed.test.mjs",
  ],
  liveCommand: null,
});
