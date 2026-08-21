import { requireOptionalNativeModule } from 'expo';

export interface TakusuAlarmsModuleType {
  canScheduleExactAlarms(): Promise<boolean>;
  requestExactAlarmPermission(): Promise<boolean>;
  scheduleEvaluatorAlarm(
    triggerAtMillis: number,
    workersUrl: string,
    rootToken: string,
    deviceId: string,
    localUrl: string,
  ): Promise<boolean>;
  cancelEvaluatorAlarm(): Promise<boolean>;
}

const TakusuAlarmsModule =
  requireOptionalNativeModule<TakusuAlarmsModuleType>('TakusuAlarms');

export default TakusuAlarmsModule;
