import { formatUnits, parseUnits, type Address } from "viem";

export function isAddressLike(v: string): v is Address {
  return /^0x[a-fA-F0-9]{40}$/.test(v);
}

export function formatAmount(raw: bigint, decimals: number, maxFrac = 6): string {
  const s = formatUnits(raw, decimals);
  const [i, f = ""] = s.split(".");
  if (!f) return i;
  return `${i}.${f.slice(0, maxFrac).replace(/0+$/, "")}`.replace(/\.$/, "");
}

export function safeParseUnits(value: string, decimals: number): bigint | null {
  try {
    if (!value.trim()) return null;
    return parseUnits(value as `${number}`, decimals);
  } catch {
    return null;
  }
}

export function nowPlusMinutes(mins: number): bigint {
  const ms = Date.now() + mins * 60_000;
  return BigInt(Math.floor(ms / 1000));
}
