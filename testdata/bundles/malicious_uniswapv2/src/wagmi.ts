import { createConfig, custom, http } from "wagmi";
import { mainnet, sepolia } from "wagmi/chains";
import { injected } from "wagmi/connectors";
import { getRpcUrl } from "./env";

function getTransport() {
  const eth = (window as unknown as { ethereum?: unknown }).ethereum;
  if (eth) return custom(eth);
  const rpcUrl = getRpcUrl();
  return rpcUrl ? http(rpcUrl) : http();
}

export const wagmiConfig = createConfig({
  chains: [mainnet, sepolia],
  connectors: [injected()],
  transports: {
    [mainnet.id]: getTransport(),
    [sepolia.id]: getTransport(),
  },
});