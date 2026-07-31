// Modal for editing a chat message in place.

import { useEffect, useState, useMemo } from 'react';
import {
  KeyboardAvoidingView,
  Modal,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';
import { useColors, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';
import { PressableScale } from '@/src/components/PressableScale';

interface EditMessageModalProps {
  visible: boolean;
  text: string;
  onClose: () => void;
  onSave: (text: string) => void;
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    overlay: {
      flex: 1,
      justifyContent: 'center',
      alignItems: 'center',
      backgroundColor: colors.overlay,
      padding: 24,
    },
    card: {
      width: '100%',
      maxWidth: 400,
      borderRadius: 16,
      padding: 20,
      gap: 16,
      shadowColor: colors.shadow,
      shadowOffset: { width: 0, height: 4 },
      shadowOpacity: 0.3,
      shadowRadius: 8,
      elevation: 8,
    },
    title: {
      fontSize: 18,
      fontWeight: '700',
    },
    inputContainer: {
      width: '100%',
      borderWidth: 1,
      borderRadius: 12,
      overflow: 'hidden',
    },
    input: {
      width: '100%',
      height: 120,
      paddingHorizontal: 12,
      paddingVertical: 10,
      fontSize: 15,
      textAlignVertical: 'top',
      backgroundColor: 'transparent',
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

export function EditMessageModal({
  visible,
  text,
  onClose,
  onSave,
}: EditMessageModalProps) {
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const [value, setValue] = useState(text);
  const canSave = value.trim().length > 0;

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
            <View
              style={[
                styles.inputContainer,
                {
                  backgroundColor: colors.surface,
                  borderColor: colors.separator,
                },
              ]}
            >
              <TextInput
                style={[
                  styles.input,
                  {
                    color: colors.black,
                  },
                ]}
                value={value}
                onChangeText={setValue}
                multiline
                autoFocus
                textAlignVertical="top"
                selectionColor={colors.brand}
                underlineColorAndroid="transparent"
              />
            </View>
            <View style={styles.actions}>
              <PressableScale
                style={styles.secondaryButton}
                onPress={() => {
                  haptic.light();
                  onClose();
                }}
              >
                <Text style={{ color: colors.gray, fontWeight: '700' }}>
                  キャンセル
                </Text>
              </PressableScale>
              <PressableScale
                style={[
                  styles.primaryButton,
                  { backgroundColor: canSave ? colors.brand : colors.grayDark },
                ]}
                disabled={!canSave}
                onPress={() => {
                  haptic.success();
                  onSave(value.trim());
                }}
              >
                <Text style={{ color: colors.white, fontWeight: '700' }}>
                  保存
                </Text>
              </PressableScale>
            </View>
          </View>
        </Pressable>
      </KeyboardAvoidingView>
    </Modal>
  );
}
