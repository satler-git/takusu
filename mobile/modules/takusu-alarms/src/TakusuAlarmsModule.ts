import { requireOptionalNativeModule } from 'expo';

export interface TakusuAlarmsModuleType {
  canScheduleExactAlarms(): Promise<boolean>;
  requestExactAlarmPermission(): Promise<boolean>;
}

const TakusuAlarmsModule =
  requireOptionalNativeModule<TakusuAlarmsModuleType>('TakusuAlarms');

export default TakusuAlarmsModule;
