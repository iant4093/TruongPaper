// vitest.config.ts
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    // Pure-logic suites run in node (docs/06 §7.1); component/store suites opt into
    // jsdom with a `// @vitest-environment jsdom` pragma at the top of the file.
    environment: 'node',
    include: ['tests/**/*.test.ts', 'tests/**/*.test.tsx'],
    setupFiles: ['tests/setup.ts'],
  },
});
