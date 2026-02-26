import { useEffect } from "react";

export function Toast(props: { message: string; onClose: () => void }) {
  useEffect(() => {
    const id = setTimeout(props.onClose, 6000);
    return () => clearTimeout(id);
  }, [props]);

  return (
    <div role="status" className="toast">
      <div className="toastInner">
        <div className="toastText">{props.message}</div>
        <button onClick={props.onClose} className="toastClose" aria-label="Close">
          ✕
        </button>
      </div>
    </div>
  );
}
