/** Angular access to a parallax engine. */

import { Injectable, InjectionToken, inject, makeEnvironmentProviders } from "@angular/core";
import type { EnvironmentProviders } from "@angular/core";

import { Connection } from "./connection";

/** Where the engine runs. */
export interface ParallaxConfig {
  /** The server's WebSocket URL, such as `wss://host/ws`. A path such as `/ws` is resolved against the page. */
  readonly url: string;
}

const CONFIG = new InjectionToken<ParallaxConfig>("parallax config");

/** Provides the parallax client for an application. */
export function provideParallax(config: ParallaxConfig): EnvironmentProviders {
  return makeEnvironmentProviders([{ provide: CONFIG, useValue: config }]);
}

function absolute(url: string): string {
  if (url.startsWith("ws://") || url.startsWith("wss://")) {
    return url;
  }
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  return `${scheme}//${location.host}${url.startsWith("/") ? url : `/${url}`}`;
}

/** The application's connection to the engine, opened on first use. It needs `provideParallax`. */
@Injectable({ providedIn: "root" })
export class ParallaxClient {
  private readonly config = inject(CONFIG);
  private connecting: Promise<Connection> | null = null;

  /** Returns the shared connection. */
  public connection(): Promise<Connection> {
    this.connecting ??= Connection.websocket(absolute(this.config.url));
    return this.connecting;
  }
}
