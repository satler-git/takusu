import { useCallback, useState, useMemo } from 'react';
import {
  ActivityIndicator,
  Alert,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { useRouter, useFocusEffect } from 'expo-router';
import { useServer } from '@/src/api/ServerProvider';
import { showError } from '@/src/api/errors';
import type { SkillRow } from '@/src/api/types';
import { useColors, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    container: { flex: 1 },
    createButton: { padding: 16 },
    createButtonText: { fontSize: 16, fontWeight: '700' },
    list: { padding: 16, gap: 12 },
    card: {
      flexDirection: 'row',
      alignItems: 'center',
      borderBottomWidth: 1,
      paddingVertical: 12,
    },
    row: { flex: 1, flexDirection: 'row' },
    text: { flex: 1, gap: 2 },
    name: { fontSize: 16, fontWeight: '700' },
    slug: { fontSize: 12 },
    desc: { fontSize: 13 },
    delete: { padding: 8 },
    deleteText: { color: colors.destructive, fontWeight: '700' },
  });

export function SkillsSettingsView() {
  const { client } = useServer();
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const router = useRouter();
  const [skills, setSkills] = useState<SkillRow[]>([]);
  const [loading, setLoading] = useState(false);

  const load = useCallback(async () => {
    if (!client) return;
    setLoading(true);
    try {
      const list = await client.listSkills();
      setSkills(list);
    } catch (e) {
      void showError(e, '読み込みに失敗');
    } finally {
      setLoading(false);
    }
  }, [client]);

  useFocusEffect(
    useCallback(() => {
      load();
    }, [load]),
  );

  function openCreate() {
    haptic.light();
    router.push('/settings/skills/edit');
  }

  function openEdit(skill: SkillRow) {
    if (skill.built_in) {
      return;
    }
    haptic.light();
    router.push({
      pathname: '/settings/skills/edit',
      params: { slug: skill.slug },
    });
  }

  async function remove(skill: SkillRow) {
    if (!client) return;
    if (skill.built_in) {
      void showError('組み込みスキルは削除できません');
      return;
    }
    Alert.alert('削除', `${skill.name} を削除しますか？`, [
      { text: 'キャンセル', style: 'cancel' },
      {
        text: '削除',
        style: 'destructive',
        onPress: async () => {
          try {
            await client.deleteSkill(skill.slug);
            haptic.success();
            await load();
          } catch (e) {
            void showError(e, '削除に失敗');
          }
        },
      },
    ]);
  }

  return (
    <View style={styles.container}>
      <Pressable onPress={openCreate} style={styles.createButton}>
        <Text style={[styles.createButtonText, { color: colors.brand }]}>
          + 新しいスキル
        </Text>
      </Pressable>
      {loading ? (
        <ActivityIndicator color={colors.brand} />
      ) : (
        <ScrollView contentContainerStyle={styles.list}>
          {skills.map((skill) => (
            <View
              key={skill.slug}
              style={[styles.card, { borderBottomColor: colors.separator }]}
            >
              <Pressable
                onPress={() => openEdit(skill)}
                disabled={skill.built_in}
                style={styles.row}
              >
                <View style={styles.text}>
                  <Text style={[styles.name, { color: colors.black }]}>
                    {skill.name}
                  </Text>
                  <Text style={[styles.slug, { color: colors.gray }]}>
                    {skill.slug}
                    {skill.built_in ? ' (built-in)' : ''}
                  </Text>
                  <Text style={[styles.desc, { color: colors.black }]}>
                    {skill.description}
                  </Text>
                </View>
              </Pressable>
              {!skill.built_in && (
                <Pressable onPress={() => remove(skill)} style={styles.delete}>
                  <Text style={styles.deleteText}>削除</Text>
                </Pressable>
              )}
            </View>
          ))}
        </ScrollView>
      )}
    </View>
  );
}
