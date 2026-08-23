import { useLocalSearchParams } from 'expo-router';
import AgentView from '@/src/views/AgentView';

export default function AgentRoute() {
  const { rescheduleTaskId, initialMessage } = useLocalSearchParams();
  return (
    <AgentView
      rescheduleTaskId={
        typeof rescheduleTaskId === 'string' ? rescheduleTaskId : undefined
      }
      initialMessage={
        typeof initialMessage === 'string' ? initialMessage : undefined
      }
    />
  );
}
