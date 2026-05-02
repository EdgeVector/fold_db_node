/// <reference types="vite/client" />
/// <reference types="vitest/globals" />

// Augments vitest's `expect` with the jest-dom matchers (toBeInTheDocument,
// toHaveAttribute, etc.). The setup file imports the runtime side at
// './src/test/setup.js'; this side-effect import makes the declarations
// visible to tsc.
import '@testing-library/jest-dom/vitest';
