import vibefiJson from "../vibefi.json";
import { mainnet, sepolia } from "wagmi/chains";

export type Addresses = typeof vibefiJson.addresses.mainnet;
export type SupportedChainId = typeof mainnet.id | typeof sepolia.id;

export const SUPPORTED_CHAIN_IDS = [mainnet.id, sepolia.id] as const;

const ADDRESSES_BY_CHAIN_ID: Record<SupportedChainId, Addresses> = {
  [mainnet.id]: vibefiJson.addresses.mainnet,
  [sepolia.id]: vibefiJson.addresses.sepolia,
};

const CHAIN_NAME_BY_ID: Record<SupportedChainId, string> = {
  [mainnet.id]: "Ethereum Mainnet",
  [sepolia.id]: "Sepolia",
};

export function isSupportedChainId(chainId: number | null | undefined): chainId is SupportedChainId {
  return chainId === mainnet.id || chainId === sepolia.id;
}

export function getAddresses(chainId: number | null | undefined): Addresses | null {
  if (!isSupportedChainId(chainId)) return null;
  return ADDRESSES_BY_CHAIN_ID[chainId];
}

export function getChainName(chainId: number | null | undefined): string {
  if (!chainId) return "Unknown";
  if (!isSupportedChainId(chainId)) return `Chain ${chainId}`;
  return CHAIN_NAME_BY_ID[chainId];
}

export function getSupportedChainText(): string {
  return `${CHAIN_NAME_BY_ID[mainnet.id]} (${mainnet.id}) and ${CHAIN_NAME_BY_ID[sepolia.id]} (${sepolia.id})`;
}
