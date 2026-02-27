import type { Address } from "viem";
import { useBalance } from "wagmi";

export function useEthBalance(address?: Address): bigint | null {
  const { data } = useBalance({
    address,
    query: { enabled: Boolean(address), refetchInterval: 10_000 },
  });
  return data?.value ?? null;
}
