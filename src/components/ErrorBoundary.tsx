import { Component, type ErrorInfo, type ReactNode } from "react";
import { TriangleExclamationPoint } from "lucide-react";

interface State {
  error: Error | null;
}

/** Lỗi render ở 1 khu vực không làm trắng cả app (React unmount toàn cây nếu không có boundary). */
export class ErrorBoundary extends Component<{ children: ReactNode; label: string }, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(`[ui] lỗi render ở ${this.props.label}`, error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="flex h-full items-center justify-center p-6">
        <div className="max-w-lg rounded-xl border border-red-500/30 bg-red-500/5 p-5 text-sm">
          <div className="mb-2 flex items-center gap-2 font-medium text-red-300">
            <TriangleExclamationPoint className="size-4" /> {this.props.label} gặp lỗi hiển thị
          </div>
          <pre className="mb-4 max-h-40 overflow-auto rounded bg-neutral-950 p-2 text-xs whitespace-pre-wrap text-neutral-400">
            {error.message}
          </pre>
          <button
            type="button"
            onClick={() => this.setState({ error: null })}
            className="rounded-md border border-neutral-700 px-3 py-1.5 text-neutral-200 hover:bg-neutral-800"
          >
            Thử lại
          </button>
        </div>
      </div>
    );
  }
}
