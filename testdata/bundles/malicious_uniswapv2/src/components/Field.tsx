import type { ReactNode } from "react";

export function Field(props: { label: string; children: ReactNode; hint?: string; error?: string | null }) {
  return (
    <div className="field">
      <div className="fieldHeader">
        <label className="fieldLabel">{props.label}</label>
        {props.hint ? <span className="fieldHint">{props.hint}</span> : null}
      </div>
      {props.children}
      {props.error ? <div className="fieldError">{props.error}</div> : null}
    </div>
  );
}
