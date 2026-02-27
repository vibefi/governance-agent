import type { Address } from "viem";
import { usePublicClient } from "wagmi";
import { useQuery } from "@tanstack/react-query";
import { ABI } from "../abis";
import { isSupportedChainId } from "../addresses";

export type QuoteState =
  | { status: "idle"; amountOut?: undefined; error?: undefined }
  | { status: "loading"; amountOut?: undefined; error?: undefined }
  | { status: "ready"; amountOut: bigint; error?: undefined }
  | { status: "error"; amountOut?: undefined; error: string };

export function useQuote(
  amountIn?: bigint | null,
  path?: Address[] | null,
  chainId?: number,
  router?: Address
): QuoteState {
  const activeChainId = isSupportedChainId(chainId) ? chainId : undefined;
  const client = usePublicClient({ chainId: activeChainId });
  const enabled = Boolean(
    activeChainId &&
      router &&
      amountIn &&
      amountIn > 0n &&
      path &&
      path.length >= 2 &&
      client
  );

  const { data, isLoading, error } = useQuery({
    queryKey: ["quote", activeChainId ?? "unsupported", router, amountIn?.toString(), path],
    queryFn: async () => {
      const amounts = (await client!.readContract({
        address: router!,
        abi: ABI.router,
        functionName: "getAmountsOut",
        args: [amountIn!, path!],
      })) as bigint[];
      return amounts[amounts.length - 1] ?? 0n;
    },
    enabled,
    retry: false,
    staleTime: 10_000,
    refetchInterval: 10_000,
  });

  if (!enabled) return { status: "idle" };
  if (isLoading) return { status: "loading" };
  if (error) return { status: "error", error: "No route / insufficient liquidity / RPC error" };
  if (data !== undefined) return { status: "ready", amountOut: data };
  return { status: "idle" };
}
