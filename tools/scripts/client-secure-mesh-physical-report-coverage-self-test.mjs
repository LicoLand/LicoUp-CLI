#!/usr/bin/env node

import { runPhysicalReportCoverageSelfTest } from "./lib/secure-mesh-physical-report-coverage.mjs";

const result = runPhysicalReportCoverageSelfTest();
console.log(JSON.stringify(result));
