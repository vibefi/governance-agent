import type { ReactNode } from "react";

export function Card(props: {
  title?: string;
  children: ReactNode;
  right?: ReactNode;
  compact?: boolean;
  noBorder?: boolean;
}) {
  const cls = ["card", props.compact ? "compact" : "", props.noBorder ? "noBorder" : ""]
    .filter(Boolean)
    .join(" ");
  return (
    <div className={cls}>
      {props.title ? (
        <div className="cardTitle">
          <div className="cardTitleText">{props.title}</div>
          {props.right}
        </div>
      ) : null}
      {props.children}
    </div>
  );
}
