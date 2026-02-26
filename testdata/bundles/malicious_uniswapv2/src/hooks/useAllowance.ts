import type { Address } from "viem";
import { useReadContract } from "wagmi";
import { ABI } from "../abis";
import { isSupportedChainId } from "../addresses";

export function useAllowance(
  token?: Address,
  owner?: Address,
  spender?: Address,
  chainId?: number
): bigint | null {
  const activeChainId = isSupportedChainId(chainId) ? chainId : undefined;
  const { data } = useReadContract({
    address: token,
    abi: ABI.erc20,
    functionName: "allowance",
    args: owner && spender ? [owner, spender] : undefined,
    chainId: activeChainId,
    query: { enabled: Boolean(activeChainId && token && owner && spender), refetchInterval: 10_000 },
  });
  return (data as bigint | undefined) ?? null;
}
