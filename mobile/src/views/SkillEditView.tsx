import { useCallback, useEffect, useState, useMemo } from 'react';
import {
  ActivityIndicator,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';
import { useRouter, useLocalSearchParams } from 'expo-router';
import { Ionicons } from '@expo/vector-icons';
import * as DocumentPicker from 'expo-document-picker';
import * as FileSystem from 'expo-file-system';
import { useServer } from '@/src/api/ServerProvider';
import { showError } from '@/src/api/errors';
import { useColors, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

function utf8ByteLength(text: string): number {
  if (typeof TextEncoder !== 'undefined') {
    return new TextEncoder().encode(text).length;
  }
  return encodeURIComponent(text).replace(/%[0-9A-Fa-f]{2}/g, 'X').length;
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
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
    saveButton: {
      paddingHorizontal: 16,
      paddingVertical: 8,
      backgroundColor: colors.brand,
      borderRadius: 8,
    },
    saveButtonText: {
      color: colors.onBrand,
      fontSize: 14,
      fontWeight: '600',
    },
    loading: {
      flex: 1,
      alignItems: 'center',
      justifyContent: 'center',
    },
    content: {
      padding: 16,
      gap: 16,
    },
    field: {
      gap: 4,
    },
    label: {
      fontSize: 13,
      fontWeight: '500',
    },
    input: {
      borderWidth: 1,
      borderRadius: 8,
      paddingHorizontal: 12,
      paddingVertical: 10,
      fontSize: 16,
    },
    bodyInput: {
      minHeight: 160,
      fontFamily: 'monospace',
      fontSize: 12,
    },
    fileButton: {
      flexDirection: 'row',
      alignItems: 'center',
      alignSelf: 'flex-start',
      gap: 8,
      marginTop: 8,
      paddingHorizontal: 12,
      paddingVertical: 8,
      borderWidth: 1,
      borderRadius: 8,
    },
    fileButtonText: {
      fontSize: 14,
      fontWeight: '600',
    },
    fileName: {
      fontSize: 12,
      marginTop: 4,
    },
  });

export function SkillEditView() {
  const { client } = useServer();
  const router = useRouter();
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const insets = useSafeAreaInsets();
  const params = useLocalSearchParams<{ slug?: string | string[] }>();
  const slugParam = Array.isArray(params.slug) ? params.slug[0] : params.slug;
  const editing = slugParam != null && slugParam.length > 0;

  const [slug, setSlug] = useState('');
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [body, setBody] = useState('');
  const [fileName, setFileName] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    if (!client || !slugParam) return;
    setLoading(true);
    try {
      const skill = await client.getSkill(slugParam);
      if (skill.built_in) {
        void showError('組み込みスキルは編集できません');
        router.back();
        return;
      }
      setSlug(skill.slug);
      setName(skill.name);
      setDescription(skill.description);
      setBody(skill.body);
    } catch (e) {
      void showError(e, '読み込みに失敗');
      router.back();
    } finally {
      setLoading(false);
    }
  }, [client, slugParam, router]);

  useEffect(() => {
    if (!slugParam) {
      setSlug('');
      setName('');
      setDescription('');
      setBody('');
      setFileName(null);
    }
  }, [slugParam]);

  useEffect(() => {
    if (slugParam) {
      load();
    }
  }, [slugParam, load]);

  function normalizeSlug(text: string) {
    return text
      .toLowerCase()
      .replace(/[^a-z0-9-_]/g, '')
      .slice(0, 64);
  }

  function validate(): string | null {
    const s = slug.trim();
    if (!editing) {
      if (!s) return 'slug は必須です';
      if (!/^[a-z0-9_-]+$/.test(s)) {
        return 'slug は小文字英数字、-、_ のみ使用できます';
      }
      if (!name.trim()) return 'name は必須です';
      if (!body.trim()) return 'body は必須です';
    } else if (!name.trim() && !description.trim() && !body.trim()) {
      return 'name, description, body のいずれかを入力してください';
    }
    if (utf8ByteLength(name.trim()) > 100)
      return 'name は100バイト以下にしてください';
    if (utf8ByteLength(description.trim()) > 500) {
      return 'description は500バイト以下にしてください';
    }
    if (utf8ByteLength(body.trim()) > 64 * 1024) {
      return 'body は64KB以下にしてください';
    }
    return null;
  }

  function fileNameToName(raw: string): string {
    const base = raw.split('.').slice(0, -1).join('.') || raw;
    return base.replace(/[_-]+/g, ' ').trim();
  }

  async function pickFile() {
    haptic.light();
    try {
      const result = await DocumentPicker.getDocumentAsync({
        type: 'text/*',
        copyToCacheDirectory: true,
      });
      if (result.canceled || result.assets.length === 0) return;
      const asset = result.assets[0];
      if (!asset) return;
      if (asset.size != null && asset.size > 64 * 1024) {
        void showError('ファイルは64KB以下にしてください');
        return;
      }
      const text = await new FileSystem.File(asset.uri).text();
      if (utf8ByteLength(text) > 64 * 1024) {
        void showError('ファイルは64KB以下にしてください');
        return;
      }
      setBody(text);
      setFileName(asset.name);
      if (!name.trim() && !editing) {
        setName(fileNameToName(asset.name));
      }
      haptic.success();
    } catch {
      void showError(
        'テキストとして読み込めませんでした。テキストファイルを選択してください。',
      );
    }
  }

  async function save() {
    if (!client) return;
    const error = validate();
    if (error) {
      void showError(error, '入力エラー');
      return;
    }
    setSaving(true);
    try {
      if (editing) {
        await client.updateSkill(slugParam, {
          name: name.trim() || undefined,
          description: description.trim(),
          body: body.trim() || undefined,
        });
      } else {
        await client.createSkill({
          slug: slug.trim(),
          name: name.trim(),
          description: description.trim(),
          body: body.trim(),
        });
      }
      haptic.success();
      router.back();
    } catch (e) {
      void showError(e, '保存に失敗');
    } finally {
      setSaving(false);
    }
  }

  function close() {
    haptic.light();
    router.back();
  }

  return (
    <View style={[styles.container, { backgroundColor: colors.white }]}>
      <View style={[styles.topBar, { paddingTop: 8 + insets.top }]}>
        <Pressable style={styles.backButton} onPress={close}>
          <Ionicons name="chevron-back" size={28} color={colors.brand} />
        </Pressable>
        <Text style={[styles.title, { color: colors.black }]}>
          {editing ? 'スキルを編集' : '新しいスキル'}
        </Text>
        <View style={{ flex: 1 }} />
        <Pressable
          style={[
            styles.saveButton,
            (saving || loading) && { backgroundColor: colors.grayDark },
          ]}
          onPress={save}
          disabled={saving || loading}
        >
          <Text style={styles.saveButtonText}>
            {saving ? '保存中…' : editing ? '保存' : '作成'}
          </Text>
        </Pressable>
      </View>

      {loading ? (
        <View style={styles.loading}>
          <ActivityIndicator color={colors.brand} />
        </View>
      ) : (
        <ScrollView
          contentContainerStyle={[
            styles.content,
            { paddingBottom: 40 + insets.bottom },
          ]}
        >
          <View style={styles.field}>
            <Text style={[styles.label, { color: colors.gray }]}>slug</Text>
            <TextInput
              style={[
                styles.input,
                { borderColor: colors.separator, color: colors.black },
              ]}
              placeholder="slug (a-z, 0-9, -, _)"
              placeholderTextColor={colors.grayLight}
              value={slug}
              onChangeText={(text) => setSlug(normalizeSlug(text))}
              editable={!editing}
              maxLength={64}
              autoCapitalize="none"
              autoCorrect={false}
            />
          </View>

          <View style={styles.field}>
            <Text style={[styles.label, { color: colors.gray }]}>name</Text>
            <TextInput
              style={[
                styles.input,
                { borderColor: colors.separator, color: colors.black },
              ]}
              placeholder="name"
              placeholderTextColor={colors.grayLight}
              value={name}
              onChangeText={setName}
            />
          </View>

          <View style={styles.field}>
            <Text style={[styles.label, { color: colors.gray }]}>
              description
            </Text>
            <TextInput
              style={[
                styles.input,
                { borderColor: colors.separator, color: colors.black },
              ]}
              placeholder="description"
              placeholderTextColor={colors.grayLight}
              value={description}
              onChangeText={setDescription}
            />
          </View>

          <View style={styles.field}>
            <Text style={[styles.label, { color: colors.gray }]}>body</Text>
            <TextInput
              style={[
                styles.input,
                styles.bodyInput,
                { borderColor: colors.separator, color: colors.black },
              ]}
              placeholder="body (markdown)"
              placeholderTextColor={colors.grayLight}
              value={body}
              onChangeText={setBody}
              multiline
              textAlignVertical="top"
            />
            <Pressable
              style={[styles.fileButton, { borderColor: colors.brand }]}
              onPress={pickFile}
            >
              <Ionicons
                name="document-outline"
                size={18}
                color={colors.brand}
              />
              <Text style={[styles.fileButtonText, { color: colors.brand }]}>
                ファイルから読み込む
              </Text>
            </Pressable>
            {fileName && (
              <Text style={[styles.fileName, { color: colors.gray }]}>
                {fileName}
              </Text>
            )}
          </View>
        </ScrollView>
      )}
    </View>
  );
}
