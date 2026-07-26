// LicensesView — foldable list of third-party licenses
// License text is duplicated in crates/takusu-cli/src/licenses.rs.
// Keep both copies in sync when adding or updating entries.
import { useEffect, useState } from 'react';
import {
  LayoutAnimation,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  UIManager,
  View,
} from 'react-native';
import { useRouter } from 'expo-router';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { Ionicons } from '@expo/vector-icons';
import { useColors } from '@/src/theme';
import { haptic } from '@/src/components/haptics';

type LicenseEntry = {
  id: string;
  title: string;
  body: string;
};

const LICENSES: LicenseEntry[] = [
  {
    id: 'mit',
    title: 'takusu',
    body: `MIT License
Copyright (c) 2025 satler`,
  },
  {
    id: 'ipadic',
    title: 'mecab-ipadic / IPADIC',
    body: `IPADIC is Copyright (c) 2002-2007 Nara Institute of Science and Technology.
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice,
   this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE
GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT
OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.`,
  },
];

export function LicensesView() {
  const router = useRouter();
  const colors = useColors();
  const insets = useSafeAreaInsets();
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  useEffect(() => {
    if (
      Platform.OS === 'android' &&
      UIManager.setLayoutAnimationEnabledExperimental
    ) {
      UIManager.setLayoutAnimationEnabledExperimental(true);
    }
  }, []);

  const toggle = (id: string) => {
    haptic.light();
    LayoutAnimation.configureNext(LayoutAnimation.Presets.easeInEaseOut);
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  };

  return (
    <View style={[styles.container, { backgroundColor: colors.white }]}>
      <View
        style={[
          styles.topBar,
          { borderBottomColor: colors.separator, paddingTop: 8 + insets.top },
        ]}
      >
        <Pressable
          style={styles.backButton}
          onPress={() => {
            haptic.light();
            router.back();
          }}
        >
          <Text style={[styles.backButtonText, { color: colors.brand }]}>
            ‹
          </Text>
        </Pressable>
        <Text style={[styles.title, { color: colors.black }]}>ライセンス</Text>
      </View>

      <ScrollView
        contentContainerStyle={[
          styles.content,
          { paddingBottom: 16 + insets.bottom },
        ]}
      >
        {LICENSES.map((license) => {
          const isExpanded = expanded.has(license.id);
          return (
            <View
              key={license.id}
              style={[
                styles.card,
                {
                  backgroundColor: colors.grayLight,
                  borderColor: colors.separator,
                },
              ]}
            >
              <Pressable
                style={styles.header}
                onPress={() => toggle(license.id)}
              >
                <Text style={[styles.headerTitle, { color: colors.black }]}>
                  {license.title}
                </Text>
                <Ionicons
                  name={isExpanded ? 'chevron-up' : 'chevron-down'}
                  size={20}
                  color={colors.gray}
                />
              </Pressable>
              {isExpanded && (
                <Text style={[styles.body, { color: colors.black }]}>
                  {license.body}
                </Text>
              )}
            </View>
          );
        })}
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
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  backButton: {
    width: 40,
    height: 40,
    borderRadius: 20,
    alignItems: 'center',
    justifyContent: 'center',
  },
  backButtonText: {
    fontSize: 28,
  },
  title: {
    fontSize: 18,
    fontWeight: '600',
    marginLeft: 8,
  },
  content: {
    padding: 16,
    gap: 12,
  },
  card: {
    borderRadius: 8,
    borderWidth: StyleSheet.hairlineWidth,
    overflow: 'hidden',
  },
  header: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    paddingHorizontal: 12,
    paddingVertical: 12,
  },
  headerTitle: {
    fontSize: 16,
    fontWeight: '600',
  },
  body: {
    paddingHorizontal: 12,
    paddingBottom: 12,
    fontSize: 13,
    lineHeight: 20,
    fontFamily: 'monospace',
  },
});
