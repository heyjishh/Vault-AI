import { Component, type ReactNode } from 'react'

interface State { failed: boolean; message: string }

export default class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { failed: false, message: '' }

  static getDerivedStateFromError(e: Error): State {
    return { failed: true, message: e.message }
  }

  render() {
    if (this.state.failed) {
      return (
        <div className="max-w-lg mx-auto mt-24 text-center px-6">
          <h2 className="font-display text-2xl tracking-tight">Something went wrong</h2>
          <p className="text-stone-500 mt-2 text-[15px]">{this.state.message}</p>
          <button
            onClick={() => window.location.reload()}
            className="mt-6 rounded-lex bg-ink text-ivory px-4 py-2 text-[14px] font-medium"
          >
            Reload LexAgent
          </button>
        </div>
      )
    }
    return this.props.children
  }
}
