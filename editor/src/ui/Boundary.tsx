// Keeps a part of the window that fails to draw from taking the rest with it. React unmounts the
// whole tree when a component throws while rendering and nothing above it catches the error: one
// value a host sends that a panel does not expect would leave a blank page. With a boundary the
// part says what failed and the rest (the transport's Stop among it) goes on working.

import { Component, type ReactNode } from "react";
import { logLocal } from "../state/logs";
import { cx } from "./cx";

interface Props {
  /** What failed, as the message names it ("The status bar"). */
  name: string;
  /** The window's own boundary: fills it and offers a reload. */
  root?: boolean;
  children: ReactNode;
}

interface State {
  error: Error | null;
}

/** The state a boundary takes when what it holds throws. */
export function failed(error: unknown): State {
  return { error: error instanceof Error ? error : new Error(String(error)) };
}

export class Boundary extends Component<Props, State> {
  override state: State = { error: null };

  static getDerivedStateFromError(error: unknown): State {
    return failed(error);
  }

  override componentDidCatch(error: unknown) {
    logLocal("error", `${this.props.name} failed to draw: ${failed(error).error!.message}`);
  }

  override render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className={cx("boundary", this.props.root && "is-root")} role="alert">
        <span className="boundary-title">{this.props.name} failed to draw</span>
        <span className="boundary-message">{error.message}</span>
        <button type="button" className="btn btn-sm btn-default" onClick={() => this.setState({ error: null })}>
          Try again
        </button>
        {this.props.root && (
          <button type="button" className="btn btn-sm btn-default" onClick={() => location.reload()}>
            Reload
          </button>
        )}
      </div>
    );
  }
}
