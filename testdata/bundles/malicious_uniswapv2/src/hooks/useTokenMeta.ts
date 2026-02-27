import { useMemo } from "react";
import type { Address } from "viem";
import { usePublicClient } from "wagmi";
import { useQuery } from "@tanstack/react-query";
import { ABI } from "../abis";
import { isSupportedChainId } from "../addresses";
import { isAddressLike } from "../utils";

export type TokenMeta = {
  address: Address;
  symbol: string;
  decimals: number;
  name?: string;
};

export function useTokenMeta(
  input: string,
  mode: "token" | "weth" = "token",
  chainId?: number,
  wethAddress?: Address
): { meta: TokenMeta | null; error: string | null } {
  const activeChainId = isSupportedChainId(chainId) ? chainId : undefined;
  const client = usePublicClient({ chainId: activeChainId });
  const addr = useMemo(() => (isAddressLike(input) ? (input as Address) : null), [input]);
  const wethMeta = useMemo<TokenMeta | null>(() => {
    if (!wethAddress) return null;
    return {
      address: wethAddress,
      symbol: "WETH",
      decimals: 18,
      name: "Wrapped Ether",
    };
  }, [wethAddress]);

  const { data, error } = useQuery({
    queryKey: ["tokenMeta", activeChainId ?? "unsupported", addr],
    queryFn: async () => {
      const [symbol, decimals, name] = await Promise.all([
        client!.readContract({ address: addr!, abi: ABI.erc20, functionName: "symbol" }) as Promise<string>,
        client!.readContract({ address: addr!, abi: ABI.erc20, functionName: "decimals" }) as Promise<number>,
        client!.readContract({ address: addr!, abi: ABI.erc20, functionName: "name" }) as Promise<string>,
      ]);
      return { address: addr!, symbol, decimals, name };
    },
    enabled: mode === "token" && Boolean(activeChainId && addr && client),
    retry: false,
    staleTime: Infinity,
  });

  if (mode === "weth") {
    if (!activeChainId || !wethMeta) return { meta: null, error: "Unsupported network" };
    return { meta: wethMeta, error: null };
  }
  if (!input.trim()) return { meta: null, error: null };
  if (!activeChainId) return { meta: null, error: "Unsupported network" };
  if (!addr) return { meta: null, error: "Invalid token address" };
  if (error) return { meta: null, error: "Could not fetch token metadata (is it an ERC-20?)" };
  return { meta: data ?? null, error: null };
}
