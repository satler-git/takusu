import { useMemo } from 'react';
import { View, Text, StyleSheet } from 'react-native';
import { type ColorSet } from '@/src/theme';

export interface DetailRowProps {
  label: string;
  value?: string;
  before?: string;
  after?: string;
  valueColor?: string;
  colors: ColorSet;
}

export function DetailRow({
  label,
  value,
  before,
  after,
  valueColor,
  colors,
}: DetailRowProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const hasDiff = before !== undefined && after !== undefined;
  return (
    <View style={styles.detailRow}>
      <Text style={[styles.detailLabel, { color: colors.gray }]}>{label}</Text>
      {hasDiff ? (
        <Text
          style={[styles.detailValue, { color: valueColor ?? colors.black }]}
        >
          <Text style={[styles.strikethrough, { color: colors.gray }]}>
            {before}
          </Text>{' '}
          → {after}
        </Text>
      ) : (
        <Text
          style={[styles.detailValue, { color: valueColor ?? colors.black }]}
        >
          {value ?? ''}
        </Text>
      )}
    </View>
  );
}

export const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    overlay: {
      flex: 1,
      backgroundColor: colors.overlay,
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
    },
    card: {
      width: '100%',
      height: '80%',
      borderRadius: 16,
      padding: 16,
      gap: 12,
    },
    header: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
    },
    statusDot: { width: 10, height: 10, borderRadius: 5 },
    title: { flex: 1, fontSize: 18, fontWeight: '700' },
    badge: { paddingHorizontal: 8, paddingVertical: 3, borderRadius: 12 },
    badgeText: { fontSize: 11, fontWeight: '700' },
    body: { flex: 1 },
    bodyContent: { gap: 14 },
    section: { gap: 6 },
    sectionTitle: { fontSize: 12, fontWeight: '600' },
    sectionBox: {
      borderWidth: 1,
      borderRadius: 10,
      padding: 10,
      gap: 8,
    },
    changeCard: {
      borderWidth: 1,
      borderRadius: 10,
      padding: 10,
      gap: 8,
    },
    changeHeader: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      flexWrap: 'wrap',
    },
    changeBadge: {
      paddingHorizontal: 8,
      paddingVertical: 3,
      borderRadius: 12,
    },
    changeBadgeText: {
      color: colors.onBrand,
      fontSize: 11,
      fontWeight: '700',
    },
    changeTarget: {
      fontWeight: '700',
      fontSize: 14,
      flexShrink: 1,
    },
    whenBlock: {
      borderWidth: 1,
      borderRadius: 8,
      padding: 8,
      paddingHorizontal: 10,
      gap: 4,
    },
    detailRow: {
      flexDirection: 'row',
      alignItems: 'baseline',
      gap: 8,
    },
    detailLabel: {
      minWidth: 56,
      fontSize: 13,
    },
    detailValue: { fontSize: 13, fontWeight: '600' },
    strikethrough: { textDecorationLine: 'line-through' },
    warningBox: {
      borderWidth: 1,
      borderRadius: 8,
      padding: 10,
      gap: 4,
    },
    nestedSectionBox: {
      borderRadius: 8,
      padding: 8,
      gap: 6,
    },
    row: {
      flexDirection: 'row',
      alignItems: 'flex-start',
      gap: 8,
    },
    rowLabel: {
      minWidth: 80,
      fontSize: 13,
      fontWeight: '600',
    },
    rowValue: { flex: 1 },
    valueText: { fontSize: 13 },
    monoText: { fontSize: 11, fontFamily: 'monospace' },
    asrItem: { gap: 2 },
    asrOriginal: { fontSize: 13, fontWeight: '600' },
    asrPurpose: { fontSize: 12 },
    footer: { gap: 10 },
    copyButton: {
      padding: 10,
      borderRadius: 8,
      alignItems: 'center',
      borderWidth: 1,
    },
    copyText: { fontSize: 13, fontWeight: '600' },
    closeButton: {
      padding: 12,
      borderRadius: 8,
      alignItems: 'center',
      justifyContent: 'center',
    },
    closeText: { fontWeight: '700' },
    emptyText: { fontSize: 13, textAlign: 'center' },
    memoryMeta: { fontSize: 11, fontFamily: 'monospace', marginTop: 4 },
    skillName: { fontSize: 16, fontWeight: '700' },
    skillSlug: { fontSize: 12, marginTop: 2 },
  });
