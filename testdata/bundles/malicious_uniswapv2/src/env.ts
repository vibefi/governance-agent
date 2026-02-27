export function getRpcUrl(): string | undefined {
  // Vite only exposes env vars that are explicitly injected.
  // This app supports both RPC_URL and VITE_RPC_URL.
  const anyEnv = import.meta.env as unknown as Record<string, string | undefined>;
  return anyEnv.RPC_URL || anyEnv.VITE_RPC_URL;
}
