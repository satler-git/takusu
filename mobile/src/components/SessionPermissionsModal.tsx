import {
  Modal,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { BRAND_COLOR, useColors } from '@/src/theme';
import type { PermissionsMap } from '@/src/api/settingsStore';
import { PermissionsEditor } from '@/src/components/PermissionsEditor';

interface Props {
  visible: boolean;
  permissions?: PermissionsMap;
  onChange: (permissions: PermissionsMap) => void;
  onClose: () => void;
}

export function SessionPermissionsModal({
  visible,
  permissions,
  onChange,
  onClose,
}: Props) {
  const colors = useColors();

  return (
    <Modal
      visible={visible}
      transparent
      animationType="fade"
      onRequestClose={onClose}
    >
      <View style={styles.overlay}>
        <Pressable style={styles.backdrop} onPress={onClose} />
        <View style={[styles.card, { backgroundColor: colors.white }]}>
          <Text style={[styles.title, { color: colors.black }]}>
            セッション権限
          </Text>
          <Text style={[styles.hint, { color: colors.gray }]}>
            ここで設定した値がProviderの権限を上書きします
          </Text>
          <ScrollView style={styles.editorScroll}>
            <PermissionsEditor permissions={permissions} onChange={onChange} />
          </ScrollView>
          <Pressable
            onPress={onClose}
            style={[styles.close, { backgroundColor: BRAND_COLOR }]}
          >
            <Text style={[styles.closeText, { color: colors.white }]}>
              閉じる
            </Text>
          </Pressable>
        </View>
      </View>
    </Modal>
  );
}

const styles = StyleSheet.create({
  overlay: {
    flex: 1,
    backgroundColor: 'rgba(0,0,0,0.4)',
    justifyContent: 'center',
    alignItems: 'center',
    padding: 20,
  },
  backdrop: {
    position: 'absolute',
    left: 0,
    right: 0,
    top: 0,
    bottom: 0,
    backgroundColor: 'transparent',
  },
  card: {
    width: '100%',
    height: '80%',
    borderRadius: 16,
    padding: 16,
    gap: 8,
  },
  editorScroll: { flex: 1 },
  title: { fontSize: 18, fontWeight: '700' },
  hint: { fontSize: 12, marginBottom: 4 },
  close: {
    marginTop: 8,
    minHeight: 44,
    borderRadius: 8,
    alignItems: 'center',
    justifyContent: 'center',
  },
  closeText: { fontWeight: '700' },
});
