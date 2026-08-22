import { NativeModule, requireNativeModule } from 'expo';

export interface StartOptions {
  port: number;
  workersUrl: string;
  rootToken: string;
  agentConfigJson?: string;
}

export interface ServerStatusResult {
  running: boolean;
  port: number;
}

export interface ScheduleOperationStatus {
  id?: string;
  status: 'running' | 'succeeded' | 'failed' | 'none';
  operation?: string;
  message?: string;
}

interface TakusuServerModuleType extends NativeModule {
  // Functions registered with Kotlin `Function` return values directly
  // (not Promise-wrapped). `await` on a non-Promise still works, but
  // `.catch()` / `.then()` on the raw return value does not — callers that
  // need Promise chaining must wrap with `Promise.resolve(...)`.
  // Functions registered with Kotlin `AsyncFunction` return a Promise.
  start(options: StartOptions): boolean;
  stop(): boolean;
  status(): ServerStatusResult;
  getLogs(): string[];
  clearLogs(): boolean;
  pushLog(line: string): boolean;
  startModelDownload(modelId: string): boolean;
  isModelCached(modelId: string): Promise<boolean>;
  runScheduleOperation(
    operation: string,
    operationId: string,
    paramsJson: string,
    workersUrl: string,
    token: string,
    port: number,
  ): boolean;
  getScheduleOperationStatus(): Promise<ScheduleOperationStatus>;
  clearScheduleOperationStatus(): boolean;
}

let moduleRef: TakusuServerModuleType | undefined | null;

function getTakusuServerModule(): TakusuServerModuleType | undefined {
  if (moduleRef === undefined) {
    try {
      moduleRef = requireNativeModule<TakusuServerModuleType>('TakusuServer');
    } catch {
      moduleRef = null;
    }
  }
  return moduleRef ?? undefined;
}

const TakusuServerModule = new Proxy({} as TakusuServerModuleType, {
  get(_target, prop) {
    const mod = getTakusuServerModule();
    if (mod == null) return undefined;
    const value = mod[prop as keyof TakusuServerModuleType];
    if (typeof value === 'function') {
      return value.bind(mod);
    }
    return value;
  },
});

export default TakusuServerModule;
