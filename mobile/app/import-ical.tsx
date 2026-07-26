// iCal import handler route — receives shared .ics files via expo-sharing
// and imports them through the server's /api/tasks/import/ical endpoint.

import { useEffect, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { useRouter } from 'expo-router';
import { Ionicons } from '@expo/vector-icons';
import * as FileSystem from 'expo-file-system';
import {
  getResolvedSharedPayloadsAsync,
  type ResolvedSharePayload,
} from 'expo-sharing';
import { useServer } from '@/src/api/ServerProvider';
import { showError } from '@/src/api/errors';
import { BRAND_COLOR, useColors } from '@/src/theme';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { haptic } from '@/src/components/haptics';

export default function ImportIcalRoute() {
  const { client } = useServer();
  const router = useRouter();
  const colors = useColors();
  const insets = useSafeAreaInsets();
  const [status, setStatus] = useState<'loading' | 'done' | 'error'>('loading');
  const [imported, setImported] = useState(0);

  useEffect(() => {
    async function processSharedPayloads() {
      if (!client) return;
      try {
        const payloads = await getResolvedSharedPayloadsAsync();
        if (payloads.length === 0) {
          setStatus('error');
          return;
        }

        let icalText = '';
        for (const payload of payloads) {
          const text = await readPayloadContent(payload);
          if (text) {
            icalText += text + '\n';
          }
        }

        if (!icalText.trim()) {
          setStatus('error');
          return;
        }

        const result = await client.importIcal(icalText);
        setImported(result.imported);
        setStatus('done');
        haptic.success();
      } catch (e) {
        showError(e, 'iCalインポートに失敗');
        setStatus('error');
      }
    }
    processSharedPayloads();
  }, [client]);

  async function readPayloadContent(
    payload: ResolvedSharePayload,
  ): Promise<string> {
    // Text-based payload: value contains the text directly
    if (payload.shareType === 'text') {
      return payload.value ?? '';
    }
    // File-based payload: read from contentUri or value (file URI)
    const uri = payload.contentUri ?? payload.value;
    if (!uri) return '';
    // For content:// URIs (Android), FileSystem.readAsStringAsync works
    // with expo-file-system's content resolver support.
    return await FileSystem.readAsStringAsync(uri);
  }

  function close() {
    router.replace('/');
  }

  return (
    <View style={[styles.container, { backgroundColor: colors.white }]}>
      <View style={[styles.topBar, { paddingTop: 8 + insets.top }]}>
        <Pressable
          style={styles.backButton}
          onPress={() => {
            haptic.light();
            close();
          }}
        >
          <Ionicons name="chevron-back" size={28} color={BRAND_COLOR} />
        </Pressable>
        <Text style={[styles.title, { color: colors.black }]}>
          iCalインポート
        </Text>
        <View style={{ flex: 1 }} />
      </View>

      <ScrollView
        contentContainerStyle={[
          styles.content,
          { paddingBottom: 40 + insets.bottom },
        ]}
      >
        {status === 'loading' && (
          <View style={styles.centerContent}>
            <Text style={[styles.statusText, { color: colors.gray }]}>
              インポート中…
            </Text>
          </View>
        )}
        {status === 'done' && (
          <View style={styles.centerContent}>
            <Ionicons name="checkmark-circle" size={64} color={BRAND_COLOR} />
            <Text style={[styles.statusText, { color: colors.black }]}>
              {imported}件のタスクをインポートしました
            </Text>
            <Pressable
              style={[styles.doneButton, { backgroundColor: BRAND_COLOR }]}
              onPress={() => {
                haptic.light();
                close();
              }}
            >
              <Text style={[styles.doneButtonText, { color: colors.white }]}>
                ホームへ
              </Text>
            </Pressable>
          </View>
        )}
        {status === 'error' && (
          <View style={styles.centerContent}>
            <Ionicons name="alert-circle" size={64} color={colors.red} />
            <Text style={[styles.statusText, { color: colors.black }]}>
              インポートに失敗しました
            </Text>
            <Pressable
              style={[styles.doneButton, { backgroundColor: BRAND_COLOR }]}
              onPress={() => {
                haptic.light();
                close();
              }}
            >
              <Text style={[styles.doneButtonText, { color: colors.white }]}>
                ホームへ
              </Text>
            </Pressable>
          </View>
        )}
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  topBar: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 8,
    paddingBottom: 8,
  },
  backButton: {
    width: 40,
    height: 40,
    borderRadius: 20,
    alignItems: 'center',
    justifyContent: 'center',
  },
  title: {
    fontSize: 18,
    fontWeight: '600',
    marginLeft: 8,
  },
  content: {
    padding: 16,
  },
  centerContent: {
    alignItems: 'center',
    gap: 16,
    paddingVertical: 48,
  },
  statusText: {
    fontSize: 16,
    fontWeight: '500',
    textAlign: 'center',
  },
  doneButton: {
    paddingHorizontal: 24,
    paddingVertical: 12,
    borderRadius: 8,
    marginTop: 8,
  },
  doneButtonText: {
    fontSize: 14,
    fontWeight: '600',
  },
});
