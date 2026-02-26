import type { Address } from "viem";
import { useReadContract } from "wagmi";
import { ABI } from "../abis";
import { isSupportedChainId } from "../addresses";

export function useErc20Balance(token?: Address, owner?: Address, chainId?: number): bigint | null {
  const activeChainId = isSupportedChainId(chainId) ? chainId : undefined;
  const { data } = useReadContract({
    address: token,
    abi: ABI.erc20,
    functionName: "balanceOf",
    args: owner ? [owner] : undefined,
    chainId: activeChainId,
    query: { enabled: Boolean(activeChainId && token && owner), refetchInterval: 10_000 },
  });
  return (data as bigint | undefined) ?? null;
}
