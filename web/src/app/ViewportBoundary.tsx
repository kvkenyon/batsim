/**
 * Error boundary around the world viewports. A WebGL failure (context
 * loss, unsupported browser) must never take down the HUD; the operator
 * keeps clocks, KPIs, and the connection state, with a clear panel where
 * the world would be.
 */

import { Component, type ReactNode } from "react";

interface Props {
  name: string;
  children: ReactNode;
}

interface State {
  failure: string | null;
}

export class ViewportBoundary extends Component<Props, State> {
  state: State = { failure: null };

  static getDerivedStateFromError(err: unknown): State {
    return { failure: err instanceof Error ? err.message : String(err) };
  }

  componentDidCatch(err: unknown): void {
    console.error(`viewport "${this.props.name}" crashed`, err);
  }

  render(): ReactNode {
    if (this.state.failure) {
      return (
        <div className="viewport-failure">
          <div className="title">graphics view unavailable</div>
          <div className="detail">
            {this.props.name}: {this.state.failure}
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
