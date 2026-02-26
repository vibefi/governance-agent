import routerAbi from "../abis/UniswapV2Router02.json";
import erc20Abi from "../abis/ERC20.json";
import factoryAbi from "../abis/UniswapV2Factory.json";
import pairAbi from "../abis/UniswapV2Pair.json";

export const ABI = {
  router: routerAbi,
  erc20: erc20Abi,
  factory: factoryAbi,
  pair: pairAbi,
} as const;
