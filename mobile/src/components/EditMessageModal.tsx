// Modal for editing a chat message in place.

import { useEffect, useState } from 'react';
import {
  KeyboardAvoidingView,
  Modal,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';
import { BRAND_COLOR, COLORS, useColors } from '@/src/theme';
import { haptic } from '@/src/components/haptics';

interface EditMessageModalProps {
  visible: boolean;
  text: string;
  onClose: () => void;
  onSave: (text: string) => void;
}

export function EditMessageModal({
  visible,
  text,
  onClose,
  onSave,
}: EditMessageModalProps) {
  const colors = useColors();
  const [value, setValue] = useState(text);

  useEffect(() => {
    setValue(text);
  }, [text, visible]);

  return (
    <Modal
      visible={visible}
      transparent
      animationType="fade"
      onRequestClose={onClose}
    >
      <KeyboardAvoidingView style={styles.overlay}>
        <Pressable style={styles.overlay} onPress={onClose}>
          <View
            style={[styles.card, { backgroundColor: colors.white }]}
            onStartShouldSetResponder={() => true}
          >
            <Text style={[styles.title, { color: colors.black }]}>
              メッセージを編集
            </Text>
            <TextInput
              style={[
                styles.input,
                {
                  color: colors.black,
                  borderColor: colors.separator,
                  backgroundColor: colors.surface,
                },
              ]}
              value={value}
              onChangeText={setValue}
              multiline
              autoFocus
              textAlignVertical="top"
              selectionColor={BRAND_COLOR}
            />
            <View style={styles.actions}>
              <Pressable
                style={styles.secondaryButton}
                onPress={() => {
                  haptic.light();
                  onClose();
                }}
              >
                <Text style={{ color: colors.gray, fontWeight: '700' }}>
                  キャンセル
                </Text>
              </Pressable>
              <Pressable
                style={[styles.primaryButton, { backgroundColor: BRAND_COLOR }]}
                disabled={!value.trim()}
                onPress={() => {
                  haptic.success();
                  onSave(value.trim());
                }}
              >
                <Text style={{ color: COLORS.white, fontWeight: '700' }}>
                  保存
                </Text>
              </Pressable>
            </View>
          </View>
        </Pressable>
      </KeyboardAvoidingView>
    </Modal>
  );
}

const styles = StyleSheet.create({
  overlay: {
    flex: 1,
    justifyContent: 'center',
    alignItems: 'center',
    backgroundColor: 'rgba(0,0,0,0.4)',
    padding: 24,
  },
  card: {
    width: '100%',
    maxWidth: 400,
    borderRadius: 16,
    padding: 20,
    gap: 16,
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 4 },
    shadowOpacity: 0.3,
    shadowRadius: 8,
    elevation: 8,
  },
  title: {
    fontSize: 18,
    fontWeight: '700',
  },
  input: {
    minHeight: 120,
    maxHeight: 240,
    borderWidth: 1,
    borderRadius: 12,
    paddingHorizontal: 12,
    paddingVertical: 10,
    fontSize: 15,
    textAlignVertical: 'top',
  },
  actions: {
    flexDirection: 'row',
    gap: 12,
  },
  secondaryButton: {
    flex: 1,
    paddingVertical: 12,
    borderRadius: 10,
    alignItems: 'center',
    justifyContent: 'center',
  },
  primaryButton: {
    flex: 1,
    paddingVertical: 12,
    borderRadius: 10,
    alignItems: 'center',
    justifyContent: 'center',
  },
});
